# 网关侧的已知事实（2026-09-04）

这份文件存在的理由很实际：这一天里 dc_llm 那边的会话换了六轮
（`dc-llm-01` → `e8` → `67` → `3c` → `31`），每换一次，上下文就丢一次，
同一段解释我讲了五遍。下面每一条都带**证据**和**复现方法**，这样下一个人
不用重新发现，也不用相信我。

**这些是 dc_llm 的行为，不是 dct 的。** 写在 dct 仓库里只是因为它没有别的家。
判断哪些该修、怎么修，是 dc_llm 那边的事。

---

## 零、2026-09-04 用户拍的三个决定

都已转给 dc_llm，**都还没落地**。写在最前面，因为下面几节的严重程度全被
它们改写了。

1. **`/pair/poll` 要发 `qwen3.8:27b`，不是 `gpt-5.4-mini`。**
   配对管道 09-03 就通了，通的是错的东西。学生配完对，配置指向的是一个
   免费账号打不动的模型。dct 侧不用改一行。
2. **5 小时窗口加到 600,000 token。** 用户原话「起码要 10 轮吧」；
   按实测一轮 56,615 算，10 轮 = 566,150，取整 60 万，是现在的 3 倍。
3. **免费额度只能用 qwen。** 用户原话「所有的 free 测试，只能用 qwen」——
   不是给免费用户更少的国际额度，是一次都没有。

**第 3 条改了第 2 条的理由**：免费用户既然碰不到国际模型，那个窗口就只
约束付费用户了。加大仍然是用户的指示，但**动机从「一个班二十人撞 429」
变成了「付费客户不该撞」**。

**第 3 条也让第五节那个 bug 更要命**：国际这条路以后只服务掏了钱的人，
而计量它的代码从来不问付没付费。同一个缺陷，位置更差了。

**第 3 条基本消掉了第六节**：`free_max_inflight = 1` 之所以是问题，是因为
Claude Code 会并发发侧信道请求——而 Claude Code 是国际专属。免费用户跑的
Qwen Code，今天两次完整运行都没撞上 `too_many_inflight`。
（只是没撞上，不等于证明了不会。）

---

## 一、Anthropic 方言这条路今天不存在

```
POST https://dc-llm.tzspace.cn/v1/messages   model=claude-sonnet-5
{"type":"error","error":{"type":"not_found_error",
 "message":"No provider is configured to serve claude-sonnet-5 over /v1/messages"}}
HTTP 404
```

额度放开之后仍然 404，**所以这是路由事实，不是限额**。所有 claude 模型都
只在 `/v1/chat/completions` 上服务（走 metaclue 那个 openai 类型的 provider）。

**后果**：Claude Code 只说 Anthropic 方言，所以它**无法直连这台网关**——
`/model` 里换什么名字都是 404。今天要用它，中间必须有一层翻译。

**能修，而且是加法不是取舍**（`dc-llm-31` 只读排查的结论）：`resolve_providers`
第二步返回**所有**在 `models_json` 里声明了该模型的启用 provider，按优先级排。
所以给同一个上游**再加一行** `type=anthropic`、优先级高于现有的 85，就能
让 `/v1/messages` 找到它，而 `/v1/chat/completions` 仍然先命中 openai 那行
（`needs_translation=False`，字节不变），anthropic 那行退居故障转移。
**把现有那行的 type 直接改掉才是取舍**——那会让 OpenAI 那条路
`needs_translation=True`，把每一个 Qwen Code 学生都塞进翻译器。

上游本身是支持的：直连 `metaclue.net:8443` 用 provider 自己的凭据打
`POST /v1/messages` model=claude-sonnet-5 → 200、真实回答、`stop_reason: end_turn`、
517 input tokens。原生，不需要 shim。

---

## 二、国内模型的 `tools` 被网关剥掉

```
POST https://dc-llm.tzspace.cn/v1/chat/completions
{"model":"qwen3.8:27b", "tools":[get_weather(city)], ...}
→ tool_calls: 无
→ prompt_tokens: 19        ← 带着工具 schema 的请求只有 19 个 token
```

**19 这个数字就是证据**：工具定义从没进过提示词。

**原因**（`dc-llm-31` 定位）：`admin-proxy/app/upstream.py` 里的
`_OLLAMA_CHAT_ALLOWED` 是一份白名单，`tools` 和 `tool_choice` 不在上面，
所以代理**返回 200 的同时静默删掉了工具**。加白名单的注释说是为了绕开
老版 Ollama 对 LangChain4j 的 400。

它的三路测量把位置钉死了：

| 打哪 | prompt_tokens | tool_calls |
|---|---|---|
| ollama 直连 `172.19.0.2:11434` | 276 | 有 |
| worker 边缘 `:11435` | 276 | 有 |
| admin-proxy（修复前） | **19** | 无 |

**状态：2026-09-04 已解决。** 修复（sha 99de406）此前只在 `gpu.tzspace.cn`
上生效，公网 `dc-llm.tzspace.cn` 仍然回 19；当天真实的 Qwen Code 两次完整
运行都打的公网名，读文件、写文件、跑命令全是真的 tool call，**工具不再被剥**。
（这两个名字解析到同一个 IP `218.95.37.16`，所以本来就不是两套部署漂移。）
**原因查清了（`dc-llm-31`）：不是重启，是当天四次部署。** 按顺序：

| sha | 干了什么 | 是不是这个症状的解 |
|---|---|---|
| `99de406` | 往 **OpenAI 路**的白名单加 `tools`/`tool_choice`/`parallel_tool_calls` | **不是。** qwen3\* 分流去了原生 `/api/chat`，根本走不到那份白名单——**记成一条红鲱鱼** |
| `59a6c2c` | **真正的解**：原生 handler 压根没转发 `tools`，Ollama 拿到的提示词里没有工具，只好用散文回答。同时补了 tool_calls 回译和流式 tool_call 增量 | **是** |
| `5d32a1d` | 上行的 assistant `tool_call.arguments` 从字符串转成对象（Ollama 收字符串会 400）；加了上游状态检查，上游拒绝不再被包成一个高高兴兴的 200 | 补充 |
| `a2d04cf` | 数组形状的 `content` 压平成 Ollama 结构要的字符串；`name` → `tool_name`；`stop`/`seed` 不再被丢掉 | 补充 |

**我那两次干净的运行打的是带 `a2d04cf` 的机器。**

值得记住的教训：`99de406` 当时看着像修复、也确实部署了，而症状没动——
因为**它修的是另一条代码路径**。「部署了修复但没好」下次先问的不是
「部署到没到」，是**「请求到底走哪条路」**。

**复现**：

```sh
curl -sS -H "authorization: Bearer <key>" -H 'content-type: application/json' \
  -d '{"model":"qwen3.8:27b","max_tokens":80,
       "messages":[{"role":"user","content":"What is the weather in Denver?"}],
       "tools":[{"type":"function","function":{"name":"get_weather",
         "parameters":{"type":"object","properties":{"city":{"type":"string"}},
                       "required":["city"]}}}]}' \
  https://dc-llm.tzspace.cn/v1/chat/completions
```

通了是 `finish_reason: tool_calls` + prompt_tokens 270 上下；坏的是 19。

---

## 三、5 小时的 token 窗口，是今天挡住课堂的那个数

```
429 「Claude」本时段额度已用完（每 5 小时 200,000 tokens），17:27 恢复
```

**算法**（`admin-proxy/app/usage_window.py`，模块头注释写得很清楚）：
不是滚动窗口，是**锚定窗口**——上一个窗口失效后的第一次调用开启新窗口，
从那一刻起算固定 5 小时。它明说了为什么不用滚动：滚动每次请求都要扫最近
五小时，要一轮一行、每查一次扫一遍，**而且永远说不出用户什么时候恢复**。

代价它也认了：「锚定的代价是有人能十分钟内把额度花光然后干等。这正是它
模仿的那些产品的做法，是性质不是缺陷。」

**统计的是输入+输出 token 的真实值**，不是换算成钱之后取整的那个。
按平台分开算，`limit = 0` 表示无限，可按账号覆盖。

**cache_read 不计入这个窗口，一个 token 都不计**（`dc-llm-31` 实测）：
计量只读上游 usage 里的 `input_tokens`，而缓存命中回来的是
`cache_read_input_tokens`。sub2api sonnet-5 上量到第 2 轮
`input_tokens = 22`、`cache_read_input_tokens = 6,501`。

**所以方向和我担心的相反**：窗口是**少算**一个带缓存的会话，不是多算，
60 万买到的比十轮**多**。但同一件事换个说法就没那么让人放心了——
**这个窗口量的不是供应商向我们收费的那个东西。** 这值得单独一个决定。

窗口能不能按平台分：能，`PlatformSubscription.window_tokens` 就是干这个的。
**20 万/5 小时是我们自己的产品设置，不是供应商的限制。**

**为什么它对课堂特别不友好**：一堂课 90 分钟正好落在一个窗口里。窗口
如果在课前就被自己用掉了，那节课全废——而学生不会知道自己「用掉了未来」。

---

## 四、三个数字

**一次「建个带按钮的网页」花 56,615 个上游 token。** 学生打的字是 24 个字符。
其中约 27k 是 Claude Code 自己的系统提示加 22 个工具的 schema，**每一轮重发**。

**对着 20 万/5 小时 = 大约 3.5 轮。** 一个会打错字、会重问的初学者撑不过十分钟。

**同一笔日额度，`qwen3.8:27b` 上约 67 轮，`gpt-5.6-sol` 上约 2 轮**
（`free_quota.py` 文件头）。**三十倍。** 这就是为什么课堂必须走国内模型——
不是省钱，是数量级不同。

---

## 四之二、Qwen Code 的真实开销（2026-09-04 实测）

第四节那三个数说的是 Claude Code。这一节是另一条路的账，**两者形状完全不同**。

同一个「诊断并修好这个 CSV 报表」任务，真的 Qwen Code（不是我们的 harness），
`qwen3.8:27b`，打 `dc-llm.tzspace.cn`。六轮，退出码 0，38 秒：

| 轮 | 输入 | 输出 |
|---|---|---|
| 1 | **28,484** | 109 |
| 2 | 580 | 155 |
| 3 | 86 | 374 |
| 4 | 186 | 104 |
| 5 | 486 | 75 |
| 6 | 170 | 552 |
| 合计 | **29,992** | **1,369** |

**第一行就是答案。** 28,484 是学生还没开口就付掉的——Qwen Code 自己的系统
提示加工具 schema。之后每轮 86–580。**这是一次性入场费，不是每轮税。**
对比 Claude Code：27k 系统提示**每轮重发**，一个任务 56,615。

一个细节别读错：llama.cpp 报的是它**真的算过**的 prompt token，KV cache 让
没变的前缀不重算。所以 2–6 轮发的是整段增长的历史，只算了新增部分。
**本地这是白送的缓存；同样的对话打按「发送量」计费的国际模型，会贵得多
而且每轮都涨。Qwen Code 便宜是因为算力是我们自己的。**

一堂课的账：**每个学生每个会话约 3 万 token，全在自己机器上算，
不碰国际计数器。**

（数字来自 Ollama 自己的 per-request 计时日志，按时间戳和客户端 IP 对上的，
不是我们的审计表——原因见下。）

**审计表记不到这些，而且比「记不到」更糟。** 那六轮在网关审计表里
`tokens_in`/`tokens_out` **全是 NULL**：qwen3 原生路的流式响应自己返回
`StreamingResponse`，绕过了读 usage 的那一层。

`dc-llm-31` 顺着查下去发现**根子更深**：原生分支自己抄了一份记账尾巴，
而那一份停在 `audit.record` 就结束了——**所以本地跑一轮既不扣额度也不扣
当天的钱包，流式非流式都一样**。同一个分支还把每一次上游拒绝都记成
status 200。

**状态：他工作树里已修**（一个共用的 `_settle_call` 同时服务原生分支和
通用路，流式那支在流耗尽时结算，包括客户端中途挂断；730 个测试绿）。
**未提交、未部署——那是用户的决定。**

---

## 四之三、范围判断：一句话就能翻，但会翻过头

第一次跑（只说「报表每个地区都印 0.00，修一下」），模型修好了 bug 1，
**找到了** bug 2（`top_regions` 升序排却标着 Top 3）、点名了正确的三个地区，
然后**明确拒绝改**，理由是「用户只报了 0.00 这个问题」。
**这是范围判断，不是能力缺口。** 它还把 `except Exception` 收窄成具体的
转换异常，比催过它的那一版更好。

第二次跑，只在提示后面加一句「也修你发现的其他 bug，不只是我报的那个」。
45 秒，退出码 0，`data.csv` 未动。它报了**三**个，bug 2 修对了
（`reverse=True`，跑出 East / South / North，正确降序）。

**但第三条是退步。** 它把「静默丢数据」单列成一个 bug，修法是
**删掉 catch**，「畸形行现在会抛可见的 ValueError/IndexError」。
同模型同任务，没催的时候它**收窄**catch，催了之后它**拆掉**catch。
后果：学生 CSV 里一行脏数据，整个报表崩。

**第三次跑，把那句限定授权的措辞真的测了**：「所有发现的 bug 都要报；
修报告的那个，其他的只在改动局部、且不删除已有错误处理时才修」。
36 秒，退出码 0，`data.csv` 未动。三个条件全中：

| 要的 | 结果 |
|---|---|
| bug 1 修好 | ✓ `csv.DictReader` + 剥千分位 |
| bug 2 修好 | ✓ `reverse=True`，降序正确 |
| 错误处理还在 | ✓ `try` / `except Exception: continue` 原样保留 |

**关键验证不是「bug 2 修没修」，是「往 CSV 里塞一行垃圾会不会崩」**
（`dc-llm-31` 提的检查点）。塞了 `North,Junk,notanumber,alsobad`：
坏行被跳过，报表照常出，**不 traceback**。第二次那一版会崩。

一个诚实的注脚：它这次**保留**了笼统的 `except Exception`，而第一次
（完全没催）是把它**收窄**成具体的转换异常。所以这句措辞买到的是安全，
不是最好的代码——「不删除已有错误处理」它照字面执行了。

**结论：可以发这句，不能发「修你发现的所有 bug」。**

---

## 五、免费额度这条路上没有「付没付费」的判断

`dc-llm-3c` 的只读排查，我没有独立复核，但它给的行号是具体的：

- `free_quota.limits_for()`（`free_quota.py:67-86`）只看两个来源：全局默认，
  和运营手写的 `quota_override:<user_id>`
- `consumer_gate.check()`（`consumer_gate.py:155-163`）对每一个 `role="user"`
  账号都计量，**从不问付没付费**
- `CreditLedger`（真的余额）、`Plan`、`PlanOrder` 三张表都存在，**这条路一张都不读**
- `apply_plan()` 只写 `PlatformSubscription`——决定「能用哪些平台」，
  **从不碰免费额度和国际调用计数**

**后果**：任何付费用户在当天第六次国际模型调用时被拒，除非有人手工写了
override。**充值充的是一个计量器从来不看的余额。**

实测支持这一条：一个**付费**账号（手机号不写在这里，问用户）撞上的却是
「今日国际模型试用次数已用完（**每日 5 次**）」。

---

## 六、并发：十个槽 vs 一个班

`free_quota.py:11`：一次国际调用可能长时间占住 **sub2api 十个并发槽之一**，
而花的钱几乎为零，**所以钱的上限挡不住免费流量把付费客户挤出去**。

这解释了「每账号 1 个在飞请求」（`too_many_inflight`）为什么存在。
但 **Claude Code 按设计会并发发侧信道请求，学生第一句话就会撞上**。
我们在自己的 shim 里串行绕过了；直接跑 agent 的学生绕不过去。

**一个班二十个学生开 Claude Code，要的是二十个槽，而上游总共十个。**
这条没人回答过：那十个槽在 anthropic 那条路上表现一样吗。

---

## 七、我今天错的两条

留在这里，因为它们比对的部分更能防止别人重走。

**错一：我说「Ollama 的 chat 模板缺 `{{ if .Tools }}`，所以工具没被渲染进提示词」。**
不成立。Ollama 服务端 0.32.15 上，qwen3.5/3.6/3.8 是 `TEMPLATE {{ .Prompt }}`
加 `RENDERER qwen3.8` / `PARSER qwen3.5`——**工具渲染搬进了内置的 Go 渲染器，
所以根本没有 `{{ if .Tools }}` 可查，缺它不是缺陷**。按我说的去重建模型会
白忙一场。真正的原因见第二节。
（那台机器上 CLI 是过时的 0.5.7，别拿它判断服务端版本。）

我说对的部分比我声称的窄：**模型会调工具**（把工具用散文写进提示词，
`qwen3.5` 一次就吐对了 `{"name":"get_weather","arguments":{"city":"Denver"}}`），
**而东西丢在网关和模型之间**。定位是 `dc-llm-31` 用三路测量做的，不是我。

**错二：我说「假上游对照实验洗清了翻译 shim」。** 下得太早。那个假上游
既没有 prefill 限制、也没有「每账号 1 个在飞」——两个真 bug 就藏在它后面：
shim 把 Claude Code 放在 `messages` 里的 `role:"system"` 当成了 `assistant`，
导致对话以 assistant 结尾，网关回 `400 does not support assistant message prefill`
（真的 Anthropic API 接受这种写法，这台网关不接受）。

**教训**：一个对照实验只能洗清它真正覆盖到的东西。用假上游证明「传输层没问题」，
证明的是「在没有那些约束的世界里没问题」。

---

## 附：dct 侧已经确认可用的部分

配对流程 2026-09-03 在生产上跑通（前四步）：签码、浏览器批准、钥匙写入两个
profile、`pair-models.toml`、`[llm]`、二次轮询回 `claimed`、未知 device_code 404。
`v0.2.7` 已发布。**dct 这边不是瓶颈**——钥匙能自动配好，但钥匙背后有没有
可用的算力，是这份文件里那些问题决定的。
