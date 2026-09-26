# dct judge：像 Jev 一样回答「选哪个、打几分、是不是」

日期：2026-09-26
状态：方向已确认，设计待用户审阅
关联：`~/work/dc/dc-vault/docs/superpowers/specs/2026-09-26-dc-vault-design.md`（「dct 替你做决定」四块的总图）；
参考：TypeSafe Jev 的 `POST /v1/systemone`、laya-serve（`github.com/NandhaKishorM/laya`，自称与 Jev 格式逐字段一致）、
Allan 的文章「A Jev-like wrapper for LLMs, including vision models」（2026-09）

## 要解决什么

用户要的 dct 是「你只说想干什么，dct 自己拍板，只有撤不回的才问」。拍板的底层是一种很窄的能力：
**给一段内容、几个问题、每个问题的选项，答出每个选项的概率。** 这正是 Jev 做的事。

用户的要求原话：「Jev 能做的事情我们都应该可以做」「dct 用来控制，dcv 用来记忆」「要支持中文」。

本设计只做**引擎和对外接口**。把它接到 dct 自己的决定上（给 DC/opencode 判断状态、替换 Telegram 那边
「是不是在等选择」、判断一步撤不撤得回）是下一份设计。

## 讨论中已经定下的事（不在本设计里再议）

- **两个都要，先做引擎和对外接口**，再一件件接到 dct 自己的决定上。
- **方法用 logprobs**，不训练模型：把选项标成字母，让模型只答一个字母，读它给每个字母的概率，在选项之间归一化。
  这个方法对看图的模型同样有效。
- **模型走 dct 现有的 `[llm]`**（也就是 DC 网关，默认 `qwen3-vl:30b-64k`：能看图、中文是母语）。
- **格式跟 Jev 逐字段一致**，现有 Jev 客户端改个地址就能用；在此之上加一个 `attachments`（图片）扩展。
- **不编概率**：拿不到 logprobs 就报错，不拿一个假的 100% 充数。
- **HTTP 服务单独跑**（`dct judge --serve`），不进守护进程。

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
- `score`：`criteria` 是**有序**的等级说明数组，2 到 20 级；每一级都必须有说明（与 laya-serve 相同，空的回 422）。
- `noul`：两个选项固定为 `false`、`true`，`criteria` 可选，用来给这两个选项配说明。返回 `true` 的概率。
- `attachments`（我们的扩展）：图片，`data:image/...;base64,` 或本机文件路径（png/jpeg/webp/gif）。
  **HTTP 接口只接受 data URL，拒绝文件路径**——否则任何能连上来的程序都能让 dct 读你磁盘上的图发给网关。
  命令行两种都行。

上限（防止一次请求把额度或内存打爆，数字取自 laya-serve）：问题 ≤ 64 个；`state` ≤ 50 000 字；
图片 ≤ 4 张；整个请求 ≤ 16 MiB（没有图片时 ≤ 2 MiB）。超了回 413 / 422，说清是哪一条。

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

## 对外接口

### 命令：`dct judge`

```
dct judge [文件]          从文件或标准输入读一个 Jev 请求，输出 Jev 回复（JSON）
dct judge --probe         用一个固定的问题打一次，只看后端给不给得出概率
dct judge --serve [--port 47900]
dct judge --token         印出 HTTP 服务的密钥
```

- 成功退出码 0；请求本身写错了退出码 2，错误说明写 stderr；后端出错退出码 1。
- 输出只有 JSON；任何给人看的话写 stderr，好让脚本直接用管道接。

### HTTP：`dct judge --serve`

- `POST /v1/systemone`：请求和回复同上。`GET /health`：`{"ok": true, "model": "..."}`，不打后端。
- **只绑 `127.0.0.1`**，默认端口 **47900**（避开 dc_workbench 的 47832）。
- **必须带 `Authorization: Bearer <密钥>`**：本机任何程序都能连上来花你的网关额度、读回判断结果。
  密钥第一次 `--serve` 时生成，存在 `~/.dct/secrets.toml` 一个 profile 不可能占用的名字下（同 `gate` 的做法），
  `dct judge --token` 印出来。比对用常数时间比较。
- 同时最多处理 8 个请求，多的回 503。请求体按上面的上限先查大小再读。
- 出错统一回 `{"error": {"code": "...", "message": "一句人话"}}`：401 没带或带错密钥；413 太大；
  422 请求写错了（指出哪个问题的哪个字段）；502 后端出错或给不出概率。**不回堆栈、不回原始系统错误。**
- 每个请求在 stderr 记一行：时间、问题数、用时、成功与否。**不记 `state` 和图片的内容。**

**为什么不放进守护进程**：放进去就要改协议版本，而旧守护进程遇到新 CLI 会让 `ps/stop/kill/prune` 吐原始报错、
升级还要重启并断掉所有会话（见 README「Building from source」）。judge 服务和会话没有任何关系，单独跑就没有这些代价。
以后如果 dct 自己的决定要用它，守护进程直接调引擎模块（同一个 crate 里的函数），不走 HTTP。

## 代码放哪儿

```
src/judge/mod.rs      请求/回复的类型、校验、上限、并发调度、usage 汇总
src/judge/prompt.rs   中英文模板、选项字母、图片组装
src/judge/logprobs.rs 从 top_logprobs 算概率（缺席字母规则、softmax、confidence、answer_confidence）
src/judge/serve.rs    HTTP 服务（鉴权、上限、并发上限、错误格式）
src/llm/resolve.rs    拆出 resolve_endpoint()
src/cli.rs / main.rs  `dct judge` 子命令、HELP 文案
```

HTTP 服务复用 `src/web/mod.rs` 已有的请求解析和常数时间比对，不再写第二份 HTTP 解析。传输层照 `llm/http.rs`
的做法注入（`with_sender`），测试不打网络。整棵依赖树仍然没有 C。

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
  都回 422 并指出位置；回复逐字段对得上 laya-serve 测试里的样例。
- **提示词**：中文 `state` 用中文模板、英文用英文模板；`state` 在最前面；图片在文字后面；
  没图片时 `content` 是字符串。
- **拿不到 logprobs**：后端回复里没有 `logprobs` → 明确报错，绝不返回概率。
- **连接**：`[llm]` 没配、是 CLI 方式、是 Anthropic 接口，三种情况各给一句能照着改的话；凭据只发往 `[llm]`
  那台主机（复用 resolve 的测试）。
- **HTTP**：没带 / 带错密钥 401；超大 413；只绑 127.0.0.1；第 9 个并发请求 503；文件路径型附件被拒；
  错误回复里不出现堆栈和原始系统错误。
- **并发**：三个问题的三个子请求确实同时发出（假后端记录到达时间）；`usage` 是三者之和。
- 所有测试用注入的假后端，不打网络。`--probe` 的真实运行结果贴进实施报告。

## 还没定、要在实施计划里回答的

- **温度 0 下的概率是否足够好用**：Qwen3-VL 在 Ollama 上的 logprobs 分布是不是有意义（而不是永远 0.999），
  要 `--probe` 之后拿几个真实例子看。
- **图片的上限**：4 张、16 MiB 是拍的；Qwen3-VL 64k 上下文里一张图占多少，实测后再调。
- **接到 dct 自己的决定上**（下一份设计）：哪些决定先用它、门槛多少、错了怎么撤回。
