# Brief for Task D7 (extracted from /Users/lei/Documents/work/dc/dc-octo/docs/superpowers/plans/2026-10-11-explore-and-learn-procedures.md)

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
### Task D7: 选项（元素 → 选择题 → 封闭词表步骤）

**目标：** 每一轮把画面变成一道 Jev 选择题：能点的元素各一项 + 固定动作，超过 20 个先用规则筛；每个选项对应一串封闭词表里的步骤。题面里没有参数值。

**Files:**
- Create: `crates/dct-explore/src/options.rs`
- Test: 同文件 `mod tests`

**Interfaces:**
- Consumes: `hands::{El, Look, StepReq}`、`platform::Platform`、`gate::tappable`
- Produces:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Tap { el: El, intent: String },                // 有字：intent = 元素上的字
    TapIcon { el: El, name: Option<String> },      // 没字：name 由大模型或用户给
    Fill { param: String, field: El, intent: String },  // tap_by_intent field → type_param param
    Attach { param: String },                      // pick_file param（选择框要已经打开）
    Scroll { down: bool },
    Wait,
    Done,
    Stuck,
}
#[derive(Debug, Clone)]
pub struct Opt { pub key: String, pub text: String, pub act: Act, pub publish: bool }

pub struct Goal<'a> {
    pub text: &'a str,                 // 用户的目标（「把这段文字发到 X」），已脱敏
    pub platform: &'a Platform,
    pub text_params: &'a [String],     // post_text、title…
    pub file_params: &'a [String],     // video、image…
}

pub struct Progress { pub filled: Vec<String>, pub attached: Vec<String>, pub published: bool, pub done_lines: Vec<String> }

/// 全部选项（不截断，给大模型和用户看）。
pub fn all_options(look: &Look, goal: &Goal, prog: &Progress, values_on_screen: &dyn Fn(&str) -> bool) -> Vec<Opt>;
/// 给 Jeff 的：最多 20 个；固定动作永远在内。
pub fn shortlist(opts: &[Opt], goal: &Goal) -> Vec<Opt>;
/// 题面：目标 + 已走的步骤 + 页面上的字（参数值换成 {名字}）。
pub fn state_text(goal: &Goal, trail: &[String], look: &Look) -> String;
pub fn question(opts: &[Opt]) -> crate::JevChoice; // instructions 固定一句，criteria = key → text
/// 一个选项变成要交给 dco 的步骤（Fill 是两步）。
pub fn steps_for(o: &Opt) -> Vec<StepReq>;
```

（`crate::JevChoice` 是 dct-explore 里和根包 `judge::ChoiceQuestion` 同形的小结构，避免 dct-explore 依赖根包；根包里写一个 `From` 转换。）

规则（写进代码注释和测试）：
- 元素有字 → `Tap`，key `e<id 去掉 e>`，text「按钮 Post」/「文字 What's happening?」；没字的检测框 → `TapIcon`，text「没有字的图标，在「<最近的有字元素>」<左/右/上/下>边」。
- `gate::tappable` 为假的不进。
- **发布词**（`platform.publish_words`，整句相等，fold 后比）的元素 → `publish: true`；只有 `text_params` 全部 `values_on_screen` 为真、`file_params` 全部在 `prog.attached` 里，才进选项。
- `Fill`：每个还没填的 `text_param` × 每个像输入框的元素（role 是 `textfield`/`textarea`/`searchfield`，或是画面上的占位字——看起来是问句、以「…」或「?」结尾、灰字由 dco 报不出来就按问句判断）出一项；**有 `file_params` 没附上时一项都不出**（文件先于文字）。
- `Attach`：每个还没附上的 `file_param` 一项，text「在打开的选择框里选文件 {video}」。
- 固定动作：`scroll_down`、`scroll_up`、`wait`、`stuck`；`done` 只在 `prog.published` 为真时出。
- `shortlist`：固定动作 + `Fill` + `Attach` 先占位；元素按分数排：发布词 +3、和目标 / 平台发布词 / 参数名的词重合 +2、role 是 button +1、在窗口中间 60% 区域 +1；同分按从上到下、从左到右。总数 ≤ 20。
- `steps_for`：`Tap` → `tap_by_intent: <intent>`；`TapIcon{name: Some(n)}` → `tap_by_intent: <n>`（`None` 不能执行，调用方要先拿到名字）；`Fill` → `[tap_by_intent: <intent>, type_param: <param>]`；`Attach` → `pick_file: <param>`；`Scroll` → `scroll: down`/`up`；`Wait` → `wait: 2`。

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hands::{El, Frac, Look};
    use crate::platform::Table;

    fn el(id: &str, t: &str, role: &str, x: f64, y: f64) -> El { El { id: id.into(), text: t.into(), role: role.into(), frac: Frac { x, y, w: 0.06, h: 0.03 }, source: "ocr".into() } }
    fn look(els: Vec<El>) -> Look { Look { snapshot_id: "s".into(), observation_id: None, scene_hash: Some("h".into()), elements: els, marks_png: None } }
    fn x_composer() -> Look { look(vec![el("e1", "Post", "button", 0.08, 0.5), el("e2", "What's happening?", "textarea", 0.45, 0.25), el("e3", "Post", "button", 0.8, 0.35), el("e4", "Follow", "button", 0.9, 0.7), el("e5", "", "image", 0.3, 0.35)]) }

    fn goal<'a>(t: &'a Table, text: &'a [String], files: &'a [String]) -> Goal<'a> { Goal { text: "把这段文字发到 X", platform: t.get("x").unwrap(), text_params: text, file_params: files } }
    fn empty() -> Progress { Progress { filled: vec![], attached: vec![], published: false, done_lines: vec![] } }

    #[test]
    fn publish_is_hidden_until_every_text_param_shows() {
        let t = Table::seed();
        let tp = vec!["post_text".to_string()];
        let before = all_options(&x_composer(), &goal(&t, &tp, &[]), &empty(), &|_| false);
        assert!(before.iter().all(|o| !o.publish), "字还没在画面上，两个 Post 都不能选");
        let after = all_options(&x_composer(), &goal(&t, &tp, &[]), &empty(), &|p| p == "post_text");
        assert_eq!(after.iter().filter(|o| o.publish).count(), 2, "字在了，两个 Post 都是候选，选哪个交给 Jeff");
    }

    #[test]
    fn files_come_before_text() {
        let t = Table::seed();
        let tp = vec!["post_text".to_string()];
        let fp = vec!["image".to_string()];
        let o = all_options(&x_composer(), &goal(&t, &tp, &fp), &empty(), &|_| false);
        assert!(o.iter().any(|o| matches!(o.act, Act::Attach { .. })));
        assert!(o.iter().all(|o| !matches!(o.act, Act::Fill { .. })), "文件没附上之前不出填字选项");
        let mut p = empty();
        p.attached.push("image".into());
        let o = all_options(&x_composer(), &goal(&t, &tp, &fp), &p, &|_| false);
        assert!(o.iter().any(|o| matches!(&o.act, Act::Fill { param, .. } if param == "post_text")));
    }

    #[test]
    fn never_tap_is_not_an_option_and_done_only_after_publish() {
        let t = Table::seed();
        let tp = vec!["post_text".to_string()];
        let o = all_options(&x_composer(), &goal(&t, &tp, &[]), &empty(), &|_| true);
        assert!(o.iter().all(|o| o.key != "e4"), "Follow 不进选项");
        assert!(o.iter().all(|o| !matches!(o.act, Act::Done)));
        let mut p = empty();
        p.published = true;
        assert!(all_options(&x_composer(), &goal(&t, &tp, &[]), &p, &|_| true).iter().any(|o| matches!(o.act, Act::Done)));
    }

    #[test]
    fn a_textless_box_is_an_icon_option_described_by_its_neighbour() {
        let t = Table::seed();
        let o = all_options(&x_composer(), &goal(&t, &[], &[]), &empty(), &|_| false);
        let icon = o.iter().find(|o| o.key == "e5").unwrap();
        assert!(matches!(icon.act, Act::TapIcon { name: None, .. }));
        assert!(icon.text.contains("没有字的图标"), "{}", icon.text);
    }

    #[test]
    fn shortlist_keeps_at_most_twenty_and_always_the_fixed_actions() {
        let t = Table::seed();
        let many: Vec<El> = (0..40).map(|i| el(&format!("e{i}"), &format!("Link {i}"), "link", 0.5, i as f64 / 40.0)).collect();
        let all = all_options(&look(many), &goal(&t, &[], &[]), &empty(), &|_| false);
        let s = shortlist(&all, &goal(&t, &[], &[]));
        assert!(s.len() <= 20);
        for k in ["scroll_down", "scroll_up", "wait", "stuck"] { assert!(s.iter().any(|o| o.key == k), "{k}"); }
    }

    #[test]
    fn state_text_never_contains_param_values() {
        let t = Table::seed();
        let l = look(vec![el("e2", "hello world from lei", "textarea", 0.4, 0.3)]);
        let s = state_text(&goal(&t, &[], &[]), &["tap_by_intent: What's happening?".into(), "type_param: post_text".into()], &l);
        // 调用方在 look 进来前已把参数值换成 {post_text}（见 Task D9 的 redact）；这里只验证题面结构
        assert!(s.starts_with("目标："));
        assert!(s.contains("已经做了："));
        assert!(s.contains("页面上的字："));
    }

    #[test]
    fn fill_becomes_tap_then_type_param() {
        let o = Opt { key: "fill_post_text_e2".into(), text: "".into(), publish: false,
            act: Act::Fill { param: "post_text".into(), field: el("e2", "What's happening?", "textarea", 0.4, 0.3), intent: "What's happening?".into() } };
        let s = steps_for(&o);
        assert_eq!((s[0].action.as_str(), s[0].arg.as_str()), ("tap_by_intent", "What's happening?"));
        assert_eq!((s[1].action.as_str(), s[1].arg.as_str()), ("type_param", "post_text"));
    }
}
```

- [ ] **Step 2: 跑测试，确认失败** — `~/.cargo/bin/cargo test -p dct-explore options` → FAIL。
- [ ] **Step 3: 实现**（按上面的规则清单写；`state_text` 格式：`目标：{goal}\n平台：{name}\n已经做了：\n- …\n页面上的字：\n[e2] What's happening?\n…`，每行一个元素，最多 200 行）。
- [ ] **Step 4: 跑全量检查** → PASS。
- [ ] **Step 5: 提交** — `git commit -m "dct-explore: options from the element list (publish only when params show, files before text, never-tap filtered)"`

**完成标准：** 测试全过；Review Focus 第 3 条的规则在这里落地。

---

