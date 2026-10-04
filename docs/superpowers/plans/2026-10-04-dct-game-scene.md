# dct game scene：看场景、指位置、点、判断有没有变（寻物游戏的第一版）  Design + Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or executing-plans. Steps use checkbox (`- [ ]`) syntax. **Speed-first** (user rule 2026-10-04): lean, do the TDD and mutation checks, no extra scope; the controller reads the diff and runs the tests instead of a separate reviewer.

**Goal:** `dct game scene [--game 名字] [--steps N] [--dry-run]` 在寻物游戏的场景里自动玩：读屏幕（文字识别，私人画面就停）→ 截图交给会看图的模型（先用网关的千问）指出下一个最值得点的位置 → 过安全检查 → 用 dco 的 `tap_at` 点 → 看有没有变化 → 记一条教学记录。把 `/private/tmp/.../scratchpad/scene_play.py` 这个已验证过的原型搬成正式命令。

**Architecture:** `llm::Prompt` 加一个可选的图片字段（`HttpBackend` 的 OpenAI 型请求用 `image_url` 内容块，命令行后端忽略并说明）。`Dco` trait 加两个有默认实现的方法：`capture`（取截图字节，走 `see` + `include_image`）和 `tap_at`（默认返回 unsupported）。`dct-game` 新增 `scene.rs`：纯函数（解析模型回答、整数万分比、不点区域、危险字检查、去重）+ 一个 `Vision` trait（给图出位置，主 crate 里用 llm 层实现）+ 循环。`src/game/scene.rs` 接命令行、配置里可选键 `no_tap`、记录写到 `~/.dct/learn/<游戏>/<日期>/steps.jsonl` 和 `png/`。

**Tech Stack:** Rust（workspace），`serde_json`，已有的 `llm`（`complete_counted_with_timeout`）、`Dco`、`profile`、`mood`。

## dco 的真实接口（2026-10-04）

- `see {window, source:"ocr", include_image:true}` 的结果里 `content` 数组除了文字块，还有一项 `{"type":"image","data":<base64 PNG，最高 900 像素高>}`。隐私闸拦住时整个 `see` 返回错误 `private_screen`（`DcoClient::see_text` 已经原样带出这个错误码；`play`/`identify` 已经会停）。
- **`tap_at` 还没有**（dc-octo 在做，按「用户 2026-10-04 的取向」：只拦隐私、金钱、Critical，没字的落点按物理档直接点；`window:{window_id}` 是它的写法，`x_bp`、`y_bp` 整数万分比，`avoid:[{x_bp,y_bp,w_bp,h_bp}…]` 最多 16 块每次带上，落点在其中 → `not_allowed`；返回 `{tapped:{at:{x_bp,y_bp}, kind:"no_text"|"text", text}}`，窗口号用 `list_windows` 现取）。**没装新版 dco 时调用会失败，dct 要用大白话说「这台章鱼还不会按位置点，等它更新」并停下（退出码 0），不要报栈。**

## Global Constraints

- 提交信息用英文，**不加任何 AI 署名或 Co-Authored-By 行**。不推送，我（控制者）来合并推送。
- 面向用户的文字用大白话中文。
- dct 代码保持通用：不出现任何游戏名；游戏专属的（不点区域、主题）只来自配置文件；危险字词表是一个命名常量（`FORBID`：hint、buy、purchase、start over、shop、store、购买、商店、提示、重新开始、`+`），注明「以后进 dcv 的规矩」。
- 发给模型的图只能是**确认是游戏画面**的图：每一步先 `see_text`，失败（含 `private_screen`）就停，**不取截图**。
- 点之前检查：落点不在 `no_tap` 区域里；落点附近（窗口宽 7%、高 9%）没有危险字的文字元素；落点在窗口内（0..=10000）；同一个落点（量化到 2%）不连着点两次。任何一项不满足就不点（本步标 `skipped`，把原因写进记录），连续 3 次 skipped 就停。
- 一次最多 `--steps` 步（默认 10，上限 100）；一步点完画面没变化（文字和数字都没变）连着 5 次停（`noop`），不是死循环。
- 记录和截图只写本机 `~/.dct/learn/`，不进仓库、不上传；每步一条 JSON（见下）。
- 旧 dco（没有 `tap_at`）、没配模型（没有 `[llm]`）、模型没回应：都停下来说人话，不崩。
- 推送前：`cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings`、`cargo +1.99.0 test --workspace --locked`（`PATH` 含 `$HOME/.cargo/bin`，`CARGO_TARGET_DIR=/private/tmp/claude-502/-Users-lei-work-dc-dc-terminal/d52f1f4f-cb49-40cc-ba88-c37403102743/scratchpad/target-199`）。已知偶发失败（机器忙）：`daemon::web_tests::enabling_starts_a_listener_and_disabling_stops_it`、`session::tests::recovering_from_a_failure_after_real_input_still_does_not_count`、`list_is_not_blocked_by_slow_create`、`zombie_reaping`，单独重跑。

## 每步的记录（`steps.jsonl`，一行一条）

```json
{"schema":1,"game":"darkness-and-flame","time_ms":0,
 "screen":{"png":"png/0001.png","texts":["5","4/6","HINT","MENU"],"numbers":[5]},
 "action":{"kind":"tap_at","x_bp":1800,"y_bp":6000},
 "teacher":"qwen","rationale":"…","model":"qwen3.8:27b","tokens_in":616,"tokens_out":66,
 "outcome":{"changed":true,"texts_after":["5","5/6","HINT","MENU"]},
 "label":"effective"}
```
`label`：`effective`（文字或数字变了）、`noop`（没变）、`skipped`（没点，带 `skip_reason`）、`error`（带 `error`）。`teacher` 是 `qwen`（以后可以是 `claude`/`user`）。

## 任务（一个子 agent 连着做，每个任务一个提交）

### Task 1: `llm::Prompt` 的图片
**Files:** `src/llm/mod.rs`、`src/llm/http.rs`（`body_for`）、`src/llm/cli.rs`（忽略并注明）；所有构造 `Prompt { .. }` 的地方补 `image_png_base64: None`。
- `Prompt` 加 `pub image_png_base64: Option<String>`。
- `body_for(Wire::Openai, ..)`：有图时 `messages[1].content` 变成数组 `[{"type":"image_url","image_url":{"url":"data:image/png;base64,<..>"}},{"type":"text","text":<user>}]`；没图时和今天**逐字节相同**（用现有测试钉住）。`Wire::Anthropic`：`{"type":"image","source":{"type":"base64","media_type":"image/png","data":<..>}}` 块在前、文字块在后。
- 测试：OpenAI 型、Anthropic 型各一，有图/没图；没图时请求体和之前完全一样；变异：有图时漏掉文字块 → 变红。
- 提交：`feat(llm): a prompt can carry one PNG image for the OpenAI and Anthropic wires; without an image the request is unchanged`

### Task 2: `Dco::capture` 和 `Dco::tap_at`
**Files:** `crates/dct-game/src/play.rs`（trait）、`crates/dct-game/src/dco.rs`（`DcoClient`）、`dco_tests.rs`
- `Dco::capture(&mut self, p) -> Result<Vec<u8>, DcoError>`：默认 `Err(unsupported)`。`DcoClient`：调 `see {window, source:"ocr", include_image:true}`，从 `result.content` 里取 `type=="image"` 那一项的 `data`，base64 解码成 PNG 字节；没有图片项 → `Err(DcoError{code:"no_image",..})`；`isError` 的 `private_screen` 原样带出错误码。（`DcoClient::call` 目前只取 `content[0].text`，加一个返回整个 `content` 的内部函数，不改现有 `call` 的行为。）
- `Region { x_bp: u16, y_bp: u16, w_bp: u16, h_bp: u16 }`（dct-game 里自己的，不依赖 dct-brain）。
- `Dco::tap_at(&mut self, p, x_bp: u16, y_bp: u16, avoid: &[Region]) -> Result<TapAt, DcoError>`：默认 `Err(unsupported)`。`TapAt { kind: String, text: String }`。`DcoClient`：先 `list_windows` 现取 `window_id`（按 `p.window` 的 `app` 匹配；取不到 → `Err(window_not_found)`），再调 `tap_at {window:{window_id}, x_bp, y_bp, avoid:[…]}`（`avoid` 最多 16 块，多了截断）；工具不存在或 `bad_request` 且消息含 `unknown tool` → 置位 `no_tap_at`（本次连接后面直接返回 `unsupported`）；`not_allowed`/`private_screen` 等错误码原样带出。
- 测试（假 dco）：capture 解码对；没有图片项的错误；`tap_at` 参数形状（window_id、x_bp、y_bp、avoid）；`unknown tool` 置位后不再发；`not_allowed` 原样带出；`avoid` 超过 16 块被截断。
- 提交：`feat(game): capture a screenshot and tap by position through dco, with a clean fallback when dco does not know tap_at`

### Task 3: `scene.rs`（纯函数和循环）
**Files:** `crates/dct-game/src/scene.rs`（新）、`lib.rs`、`mood.rs`（`for_scene_stop` 之类，可选）
- 纯函数（全部带测试）：
  - `parse_pick(text: &str) -> Option<Pick>`：从模型回答里找第一个 `{`..最后一个 `}`，解析 `{"name","x","y","why"}`；`x`、`y` 必须是 0..=1 的数；去掉 ```json 围栏和 `<think>…</think>`；解析不了 → `None`。`Pick { name, x, y, why }`。
  - `to_bp(x: f64) -> u16`（四舍五入，夹在 0..=10000）。
  - `in_regions(x_bp, y_bp, &[Region]) -> bool`（含边界）。
  - `near_forbidden(elements: &[Element+frac], x, y) -> Option<String>`：窗口宽 7%、高 9% 内的文字元素文字（转小写）含 `FORBID` 任一词（`+` 只算整条文字就是 `+` 的）→ 返回那条文字。
  - `quantise(x_bp, y_bp) -> (u16,u16)`（2% 一格）。
- `trait Vision { fn pick(&self, png: &[u8], history: &[(u16,u16)]) -> Option<VisionAnswer> }`，`VisionAnswer { pick: Option<Pick>, raw: String, model: String, tokens: Option<(u64,u64)> }`。
- `scene(dco, clock, p, vision, o: &SceneOptions, sink) -> SceneSummary`：循环按上面「Global Constraints」的步骤；`SceneOptions { max_steps, dry_run, no_tap: Vec<Region> }`；停止原因枚举 `SceneStop`：`StepsDone`、`PrivateScreen`、`NoTapAt`（旧 dco）、`NoVision`（模型没回应/没配）、`Skipped3`、`Noop5`、`Dco(DcoError)`、`DryRun`。dry-run：只做到「过检查、写一条 `label:"dry_run"` 记录」，不调用 `tap_at`。章鱼：开局不发；点之前发 `show_status_with("think", Some("看场景"), None)`，点完发 `look`；结束按 `for_stop` 风格发（私人画面 → wait）。
- 效果判断：点前读一次 `see_text`（texts、numbers），点后等 `settle`（用 `swipe` 不适用；这里 `clock.sleep_ms(1500)` 再读一次 `see_text`）：文字列表或数字变了 → `effective`，否则 `noop`。
- 测试（假 dco + 假 Vision）：`parse_pick` 各种回答（围栏、think、越界、缺字段）；`to_bp` 边界；`in_regions` 边界；`near_forbidden`；去重；一步成功写出完整记录；不点区域内的选择 → skipped，不调用 tap_at；危险字 → skipped；连续 3 次 skipped → `Skipped3`；连续 5 次 noop → `Noop5`；旧 dco（tap_at unsupported）→ `NoTapAt` 且一步都没点；`private_screen` → `PrivateScreen` 且**没调用 capture**；dry-run 不调 tap_at；最大步数。变异：去掉不点区域检查 → 对应测试变红；把 private_screen 的检查挪到 capture 之后 → 「没调用 capture」测试变红。
- 提交：`feat(game): scene loop that screens, asks a vision model for one target, checks it, taps and records`

### Task 4: 命令、模型接线、记录、配置、说明卡
**Files:** `src/game/scene.rs`（新）、`src/game/mod.rs`、`src/game/cli.rs`（`dct game scene` 分流，和 `identify`、`ask-bench` 一样放在 `parse` 之前）、`src/game/profile.rs`（可选键 `no_tap`、`theme` 已有）、`src/main.rs`（`--help` 一行）、`src/game/skill.md`
- `LlmVision`：用 `crate::cli::load_llm_backend()`，`Prompt { system, user, max_tokens: 300, image_png_base64: Some(..) }`，用 `complete_counted_with_timeout`（超时 60 秒）；系统提示和用户提示用原型里验证过的那段（`scene_play.py` 里的 `q`，要求只回 JSON `{name,x,y,why}`、不要选菜单/按钮/物品栏/提示/金币、列出已点过的位置不要重复）；失败/没配 → `None`，命令打印「没开大模型，没法看场景」或「大模型没回应」并停（退出码 0）。
- 缩图：把 PNG 缩到最高 1000 像素再发（用 `image` 之类已有依赖；如果没有，沿用 dco 给的 900 像素图，不另缩）。
- 记录：`~/.dct/learn/<game>/<YYYY-MM-DD>/steps.jsonl` 和 `png/NNNN.png`（目录权限 0700），写不了就停并说明；每步的图保存的是**发给模型的那一张**。
- 配置键 `no_tap = [{ x = 0.78, y = 0.0, w = 0.22, h = 0.2 }, …]`（窗口 0–1，最多 16 块，转成整数万分比；缺省空），读进 `SceneOptions`。**不要给内置的 candy-crush 或本地 `level-17xx.toml` 加任何东西**。
- 命令输出：每步一句大白话（「第 3 步：点了『木箱』，画面变了」「这一步没点：落点附近有『提示』」），结束一句停止原因，最后「这次问了大模型 N 次，一共用了约 X 个 token」（复用 `LlmAdvisor` 同样的说法；这里自己累计）。
- `skill.md` 加一小节「寻物游戏」：`dct game scene`，需要最新版 dco，模型只能看到确认过的游戏画面，不点的区域在配置里写 `no_tap`。
- 测试：命令行解析（`--game`、`--steps` 1..=100、`--dry-run`、未知参数）；`no_tap` 解析（越界、超过 16 块）；记录文件落盘（临时目录，`HOME` 换成临时目录，断言 `steps.jsonl` 和 `png/0001.png` 存在、权限）；`LlmVision` 用假后端（返回 JSON → 解析出 pick；返回乱码 → `None`）；`dct game scene --dry-run` 整条命令用假 dco + 假后端的集成测试（参照 `tests/game_cli.rs` 的写法，别用真 dco）。
- 提交：`feat(game): dct game scene command with a gateway vision model, no_tap regions from the profile and teaching records on disk`

## 不做

- 真正的 dcv 对接（`no_tap` 以后来自 dcv 的规矩）；训练；`claude` 当老师；`wait_idle` 判断稳定（等 dco 的 `tap_at` 返回里带 changed 再换）；小物件的专门处理；任何 dco 那边的改动。

## Review Focus（给我自己读 diff 时对照）

1. `private_screen`/任何 `see_text` 失败时，**绝对不取截图、不调模型、不点**。→ Task 3 的测试。
2. 旧 dco 的 `unknown tool: tap_at`：说人话并停，一步都没点，不当成「点了没反应」。→ Task 2/3。
3. 模型回的位置在 `no_tap` 里或危险字旁边：不点，写 skipped 的原因，不崩。
4. 记录目录权限 0700，不进仓库，图是发给模型的那一张。
5. 没有 `[llm]`：说人话，退出码 0，不崩。
