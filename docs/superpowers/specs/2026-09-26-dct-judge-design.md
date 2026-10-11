# dct judge：像 Jev 一样回答「选哪个、打几分、是不是」

日期：2026-09-26
状态：方向已确认，设计待用户审阅（已按「dct 去控制别的软件，而不是让别的软件来连 dct」收窄：第一版不开 HTTP 服务）
关联：`~/work/dc/dc-vault/docs/superpowers/specs/2026-09-26-dc-vault-design.md`（「dct 替你做决定」四块的总图）；
参考：TypeSafe Jev 的 `POST /v1/systemone`、laya-serve（`github.com/NandhaKishorM/laya`，自称与 Jev 格式逐字段一致）、
Allan 的文章「A Jev-like wrapper for LLMs, including vision models」（2026-09）

## 要解决什么

用户要的 dct 是「你只说想干什么，dct 自己拍板，只有撤不回的才问」。拍板的底层是一种很窄的能力：
**给一段内容、几个问题、每个问题的选项，答出每个选项的概率。** 这正是 Jev 做的事。

用户的要求原话：「Jev 能做的事情我们都应该可以做」「dct 用来控制，dcv 用来记忆」「要支持中文」。

本设计只做**引擎和 `dct judge` 命令**。把它接到 dct 自己的决定上（给 DC/opencode 判断状态、替换 Telegram 那边
「是不是在等选择」、判断一步撤不撤得回）是下一份设计。

## 讨论中已经定下的事（不在本设计里再议）

- **两个都要，先做引擎**，再一件件接到 dct 自己的决定上。
- **不开让别的软件来连 dct 的 HTTP 服务。** 用户的方向是「dct 去控制别的软件」：别人不会为 dct 改自己的软件，
  所以 dct 主动去连别人已经开好的门（MCP、HTTP 接口、命令行、AppleScript、浏览器），那是另一份设计。
  judge 在这里的角色是**给 dct 自己做决定用的引擎**，外加一条给人和脚本用的命令。
- **方法用 logprobs**，不训练模型：把选项标成字母，让模型只答一个字母，读它给每个字母的概率，在选项之间归一化。
  这个方法对看图的模型同样有效。
- **模型走 dct 现有的 `[llm]`**（也就是 DC 网关，默认 `qwen3-vl:30b-64k`：能看图、中文是母语）。
- **格式跟 Jev 逐字段一致**，现有 Jev 客户端改个地址就能用；在此之上加一个 `attachments`（图片）扩展。
- **不编概率**：拿不到 logprobs 就报错，不拿一个假的 100% 充数。

## 请求：跟 Jev 一样

```json
{
  "state": "要判断的内容（字符串，或任意 JSON，JSON 会被原样序列化）",
  "questions": {
    "dept":    { "type": "choice", "instructions": "哪个部门处理？",
                 "criteria": { "billing": "付款、退款", "tech": "故障、报错", "other": null } },
    "urgency": { "type": "score",  "instructions": "有多急？",
                 "criteria": ["不急", "尽快", "卡住了"] },
    "churn":   { "type": "noul",   "instructions": "对方是不是在说要走？",
                 "criteria": { "false": "没提要走", "true": "明确说要取消或离开" } }
  },
  "attachments": ["data:image/png;base64,....", "~/Desktop/shot.png"],
  "model": "可选；给了且不是空的，就替换 [llm] 里的 model"
}
```

- `choice`：`criteria` 是「选项名 → 说明（可为 null）」，**2 到 20 个**（字母 A–T）。选项名原样当答案回传。
- `score`：`criteria` 是**有序**的等级说明数组，2 到 20 级；每一级都必须有说明（与 laya-serve 相同，空的就报错）。
- `noul`：两个选项固定为 `false`、`true`，`criteria` 可选，用来给这两个选项配说明。返回 `true` 的概率。
- `attachments`（我们的扩展）：图片，`data:image/...;base64,` 或本机文件路径（png/jpeg/webp/gif）。
  命令行两种都行。

上限（防止一次请求把额度或内存打爆，数字取自 laya-serve）：问题 ≤ 64 个；`state` ≤ 50 000 字；
图片 ≤ 4 张；整个请求 ≤ 16 MiB（没有图片时 ≤ 2 MiB）。超了就报错，说清是哪一条。

## 回复：跟 Jev 一样

```json
{
  "answers": {
    "dept":    { "type": "choice", "choice": "billing",
                 "probabilities": { "billing": 0.91, "tech": 0.06, "other": 0.03 },
                 "confidence": 0.63, "answer_confidence": 0.91 },
    "urgency": { "type": "score", "score": 1.42,
                 "legend": { "0": "不急", "1": "尽快", "2": "卡住了" },
                 "probabilities": { "0": 0.08, "1": 0.42, "2": 0.50 },
                 "confidence": 0.13, "answer_confidence": 0.50 },
    "churn":   { "type": "noul", "noul": 0.87, "confidence": 0.87, "answer_confidence": 0.87 }
  },
  "usage": { "input_tokens": 1830, "output_tokens": 3 }
}
```

字段含义与 laya-serve 相同：

- `probabilities`：归一化后的分布，四位小数。
- `choice`：概率最大的选项名。`score`：期望等级 `Σ i·pᵢ`。`noul`：`true` 的概率。
- `confidence`：`choice`/`score` 是 `1 − 归一化熵`（熵除以 `ln k`）；`noul` 是 `max(p, 1−p)`。
- `answer_confidence`：报出的那个答案的概率（`score` 取概率最大那一级）。**这是调用方该拿来设门槛的数。**
- `usage`：所有子请求的 `prompt_tokens`、`completion_tokens` 之和。
- laya 的 `action` 字段不回：它自己的文档说那个数「还没有可用的信号」。

## 怎么问：一个问题一次请求

每个问题单独发一次 OpenAI 兼容的 `POST {base}/chat/completions`，几个问题**并发**发。
**`state` 永远放在提示词最前面**，同一次请求里的几个问题共享同一段前缀，支持前缀缓存的后端（vLLM、llama.cpp）
只算一遍。

提示词（`state` 里有中文就用中文模板，否则用英文模板；两份模板逐句对应）：

```
内容：
{state}

问题：{instructions}
选项：
[A] billing：付款、退款
[B] tech：故障、报错
[C] other

只回答最合适的那个选项的字母。
```

请求体：

```json
{ "model": "...", "messages": [{ "role": "user", "content": [ {"type":"text","text": "..."},
                                                             {"type":"image_url","image_url":{"url":"data:..."}} ] }],
  "max_tokens": 1, "temperature": 0, "logprobs": true, "top_logprobs": 20,
  "reasoning_effort": "none" }
```

- 有图片时 `content` 是数组（文字在前、图片在后）；没图片时是字符串。
- `reasoning_effort: "none"` 是为了关掉「先思考」：会思考的模型第一个 token 是 `<think>` 之类，而不是字母，
  整个方法就失效了。**这一条必须实测**（见「动手的第一步」）。

### 从 logprobs 算出概率

读回复里 `choices[0].logprobs.content[0].top_logprobs`（每项 `{token, logprob}`）：

1. token 先去掉首尾空白再比对（有的后端给的是 `" A"`）。同一个字母出现多次取最大。
2. 选项字母里至少有一个出现；一个都没有就报错「模型没按字母回答」，并带上它实际给的第一个 token
   （比如 `<think>`），方便判断是不是思考没关掉。
3. 有字母没出现在 top 列表里时：它的概率不可能比列表里最后一名还高。把「所有缺席字母都按最后一名的概率算」
   得到的总量占比 **< 10⁻⁶** 才允许把它们记成 0；否则报错「后端没给出足够的备选，概率不可靠」。
   （这条规则来自 Allan 的文章：宁可报错，也不把一个可能不小的概率悄悄算成 0。）
4. 在选项字母之间做 softmax（先减最大值），得到 `probabilities`。

**拿不到 logprobs 就报错**，不退回「让模型写 JSON 概率」之类的替代：那种数字是模型编的，和 dct
「宁可显示 `—` 也不编状态」是同一条规矩。

## 用哪个模型、凭据怎么来

- 读 `~/.dct/config.toml` 的 `[llm]`。**只支持 `transport = "http"` 且接口是 OpenAI 兼容的**；
  Anthropic 接口没有 logprobs，CLI 方式拿不到原始回复——这两种情况直接说「judge 需要一个 OpenAI 兼容的
  HTTP 连接」，并指出该改 `config.toml` 的哪一行。
- 没有 `[llm]` 段：说「judge 需要先配好 `[llm]`」，并给出 DC 网关的写法。**judge 不会替用户打开 `[llm]`**
  （`config.rs` 开头的隐私边界照旧成立：发出去的是调用方给的内容，不是终端画面，但开不开仍由用户决定）。
- **凭据和目的地规则全部复用 `llm/resolve.rs`**，不另写一套。现在 `resolve()` 直接返回搭好的
  `Arc<dyn Backend>`，judge 要的是更底层的「地址 + 接口类型 + 模型 + 凭据」。所以做一处小拆分：
  抽出 `resolve_endpoint()` 返回这四样，原来的 `resolve()` 改成在它之上搭 `Backend`；**现有调用方一行不改**，
  「按目的地主机判断凭据」那条规矩只在 `resolve_endpoint()` 里写一次。
- 超时：每个子请求连接 5 秒、总共 60 秒（`.timeout()` 和 `.timeout_connect()` 都设，理由见 `llm/http.rs`）。

## 命令

### 命令：`dct judge`

```
dct judge [文件]          从文件或标准输入读一个 Jev 请求，输出 Jev 回复（JSON）
dct judge --probe         用一个固定的问题打一次，只看后端给不给得出概率
```

- 成功退出码 0；请求本身写错了退出码 2，错误说明写 stderr；后端出错退出码 1。
- 输出只有 JSON；任何给人看的话写 stderr，好让脚本直接用管道接。


## 代码放哪儿

```
src/judge/mod.rs      请求/回复的类型、校验、上限、并发调度、usage 汇总
src/judge/prompt.rs   中英文模板、选项字母、图片组装
src/judge/logprobs.rs 从 top_logprobs 算概率（缺席字母规则、softmax、confidence、answer_confidence）
src/llm/resolve.rs    拆出 resolve_endpoint()
src/cli.rs / main.rs  `dct judge` 子命令、HELP 文案
```

传输层照 `llm/http.rs` 的做法注入（`with_sender`），测试不打网络。守护进程以后要用它，直接调这个模块里的函数。整棵依赖树仍然没有 C。

## 动手的第一步：先验证，不先写引擎

这台机器上**还没有网关密钥**；网关背后是 Ollama 0.32.4，dc_llm 的代码和文档里**没有任何地方提到 logprobs**，
请求还要经过网关的 Caddy。所以第一个任务只做 `dct judge --probe`，拿到密钥之后先跑它，回答四个问题：

1. 回复里有没有 `logprobs.content[0].top_logprobs`？
2. `reasoning_effort: "none"` 之后，第一个 token 是不是字母（而不是 `<think>`）？
3. `top_logprobs` 给了几个备选？（决定缺席字母规则会不会经常报错）
4. 一次问题要多久？

**退路**（按顺序）：网关管理页把后端切到 vLLM（肯定支持 logprobs）；或者开发期在本机用 llama.cpp
（`/chat/completions` 支持 logprobs，文章作者用的就是它）。验证不通过，引擎的其余部分不开工。

## 和 Jev 比，哪些已经一样、哪些还差

| | Jev | dct judge |
|---|---|---|
| 请求 / 回复格式 | — | 一样（加了 `attachments`） |
| 三种题型 | ✓ | ✓ |
| 中文 | 官方未强调 | ✓（Qwen 母语，中文模板） |
| 看图 | ✗ | ✓（Qwen3-VL） |
| 一题 33 ms | ✓（专用小模型） | ✗ 一题要走一次网关请求，秒级 |
| 概率经过校准 | ✓（专门训练） | ✗ 大模型的原始概率偏自信；`answer_confidence` 能排序，不能当真概率用 |
| 每题最多 255 个选项 | ✓ | ✗ 最多 20（字母 A–T） |

后三行是**已知差距**，不在这一版里补。补的路子：速度靠网关前缀缓存 + 本机小模型；校准靠用 dcv 里攒下的
「你拍过板的决定」拟合温度；选项多靠先粗筛到 20 个以内再问（laya 的 `predict_shortlist` 同一个思路）。

## 怎么测

- **算概率**：给定 top_logprobs，分布、`choice`、期望等级、`noul`、两种 confidence 都对；token 带空格照样认；
  同一字母多次取最大；缺席字母占比 < 10⁻⁶ 记 0、≥ 10⁻⁶ 报错；一个字母都没有时报错并带上第一个 token。
- **格式**：请求里每一种写错（类型不认识、选项 < 2 或 > 20、score 某级没说明、问题 > 64、`state` 过长）
  都报错并指出位置（命令退出码 2）；回复逐字段对得上 laya-serve 测试里的样例。
- **提示词**：中文 `state` 用中文模板、英文用英文模板；`state` 在最前面；图片在文字后面；
  没图片时 `content` 是字符串。
- **拿不到 logprobs**：后端回复里没有 `logprobs` → 明确报错，绝不返回概率。
- **连接**：`[llm]` 没配、是 CLI 方式、是 Anthropic 接口，三种情况各给一句能照着改的话；凭据只发往 `[llm]`
  那台主机（复用 resolve 的测试）。
- **并发**：三个问题的三个子请求确实同时发出（假后端记录到达时间）；`usage` 是三者之和。
- 所有测试用注入的假后端，不打网络。`--probe` 的真实运行结果贴进实施报告。

## 还没定、要在实施计划里回答的

- **温度 0 下的概率是否足够好用**：Qwen3-VL 在 Ollama 上的 logprobs 分布是不是有意义（而不是永远 0.999），
  要 `--probe` 之后拿几个真实例子看。
- **图片的上限**：4 张、16 MiB 是拍的；Qwen3-VL 64k 上下文里一张图占多少，实测后再调。
- **接到 dct 自己的决定上**（下一份设计）：哪些决定先用它、门槛多少、错了怎么撤回。

## 2026-10-11 第一版后端：jeff-serve（Jev HTTP）

「探索并学会流程」要一个很快的「选哪个」判断者（Jeff）。第一版的接法是：**dct 的 judge 用 HTTP 调 Jeff 自带的 `jeff-serve`（`POST /v1/systemone`，Jev 格式）**，不在 dct 进程里跑 ONNX，也不走上面的 OpenAI 兼容 + logprobs。后端本来就是可插拔的，这是第一个后端。

### 为什么

1. **Jeff 不是普通的「生成字」模型。** 它在 Qwen3.5-0.8B 上加了自己的输出层（`readout.safetensors` + `decision_config.json`），直接给每个选项的概率。llama.cpp、mlx_lm 那类 OpenAI 兼容服务器加载不了这个输出层，「让模型只答一个字母、读 logprobs」对 Jeff 用不上。
2. **`jeff-serve` 已经存在，说的就是本设计要回的 Jev 格式**（`state` / `questions` / `choice` 题，回 `probabilities` + `choice`）。dct 这边只是 HTTP 客户端，几乎不用翻译。laya-serve、TypeSafe Jev 说同一种格式，同一个后端都能连。
3. **ONNX 不是最小的一步**：Jeff 的自定义输出层加 Qwen3.5 的混合注意力能不能导出 ONNX、导出后准不准都没验证过；dct 现在整棵依赖树没有 C，进程内 ONNX 要另引 onnxruntime。这些留到「正式给用户」那一版。

### 和上文的三处差别

- 请求体必须带 `model`（`"jeff-latest"`）；图片字段叫 `images` 不叫 `attachments`，第一版不发图（Jeff 的 MLX 后端只收文字，`/health` 里 `modalities: ["text"]`）。
- **回复里没有 `answer_confidence`**，`confidence` 的公式也和上文不同。dct 自己从 `probabilities` 重算：先归一化，`answer_confidence = probabilities[choice]`，`choice` 也按重算后概率最大的取，不信回复里的。门槛只看这个数。回复里选项缺一个、多一个、概率为负或总和为 0，一律当「读不懂」，不猜。
- 服务同一时间只接一个请求，忙时回 529 + `Retry-After: 1`：dct 串行发，529 等 1 秒重试一次，仍忙就报 `Busy`，往上交给调用方当「Jeff 失败」。

### 配置与命令

`~/.dct/config.toml` 的 `[judge]`（不写 = 关）：`base_url`（必填，如 `http://127.0.0.1:8765`）、`backend`（只有 `"jev"`）、`model`（默认 `jeff-latest`）、`api_key_env`（有就从这个环境变量读密钥作 Bearer，只发给 `base_url` 那台主机，值不写进配置）、`threshold`（默认 0.8）、`timeout_ms`（默认 5000）。

`dct judge [文件]` 只支持 `choice` 题（`score` / `noul` 报「这一版只做 choice」，退出码 2）；`dct judge --probe` 发一道固定的中文 GUI 题。没配 `[judge]` 时 stderr 给出该写的那一行，退出码 1。请求写错 2，后端错 1。选项仍是 2–20 个。

### 这一版不做

- 产品规则「模型不打包进程序、用到才下载、正式版用 ONNX Runtime」：这一版只在开发机和试点机上跑，`jeff-serve` 由人手动起，dct 不自动起、不自动下载权重。
- OpenAI 兼容 + logprobs 后端（给能出 logprobs 的大模型用）、`score` / `noul` 的服务端实现、图片附件。
- Jeff 微调：这一版只把题存下来（见下面「训练题」）。dct 自己训练 Jeff 的设计另写：`2026-10-11-dct-jeff-auto-training-design.md`。

没配 `[judge]` 或 Jeff 没起来：跳过 Jeff，直接大模型再到用户，每处模型用法都要有不靠模型的退路。

### 实现以后补的几件事（2026-10-11，`explore-and-learn` 分支）

- **选项顺序原样送到服务端。** 请求体是按顺序手写出来的 JSON 文本，不经会排序的 `Map`（没开 serde_json 的 `preserve_order`，那会改动 dct 别处的 JSON 输出）。`dct judge [文件]` 也按文件里写的顺序读；只有同一道题里选项名写重了，才退回按名字排序。
- 服务端回 422（题不合它的要求，例如选项比模型能接的多）→ 当「请求有误」，不重试。
- 配了 `api_key_env` 时，明文 `http://` 只能发往本机（127.0.0.1 / localhost / ::1），别的主机必须 https，否则不发请求。
- **接进了 `dct do` 的逐级往上**：Jeff 的 `answer_confidence` ≥ `threshold` 才算数；不够、没答、或它选的那一下没让画面变 → 同一画面上问 `[llm]` 的大模型（最多 2 次）→ 用户。危险与否永远由规则（dct 的停下规矩、禁点词、dco 的按钮分档）判断，不看任何模型的把握。
- **训练题**：`dct do` 摸索时每答一道题（照做后到了新画面）就往 `~/.dct/explore/answers.jsonl` 追加一行（0600），参数值写盘前已换回 `{名字}`。字段：`ts_ms`、`run_id`、`platform`、`state`（题面）、`options`（这一轮全部选项的 `[key, 文字]`，可能多于发给 Jeff 的 20 个）、`jeff`（Jeff 当时的 `[key, 把握]`，没有就 `null`）、`answer`、`by`（`jeff` / `model` / `user`）、`corrects_jeff`（不是 Jeff 答的、而 Jeff 在这个画面上选过别的）。

### 真跑过的数（dc-octo `docs/experiments/2026-10-11-jeff-serve-d4.md`）

- 权重：Jeff **v1.2**，`jeff-legacy/Jeff-Qwen3.5-0.8B`，revision `d86f657`（全称 `d86f657f503245be5291e3c69e573bf48033aa50`），约 **1.6 GB**（不下演示视频），`/health` 的 `max_options` 是 **254**。原来的 `mstrasser/Jeff-Qwen3.5-0.8B` 已经搬到 `jeff-legacy` 名下；新的 v1.3 底座 `mstrasser/jeff-base` 要配 adapter 用，零样本仍用 v1.2。
- 开发机 M5 Max（**不是** M2 16GB 基准机）：短题 p50 约 13 ms，上千 token 的新长度第一次一两百到三百多毫秒；GUI 题英文 71.7%、中文 58.3%；把握 ≥ 0.8 的 45 次对 44 次，所以门槛起始 0.8。M2 上还没量。
- `dct judge --probe` 还没对着真的 `jeff-serve` 跑过（D4 是直接发 HTTP 量的）。
