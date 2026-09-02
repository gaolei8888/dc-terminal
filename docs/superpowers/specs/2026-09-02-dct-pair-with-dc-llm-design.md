# dct 配对 dc-llm：让小白装完就能用 —— 设计

**状态：** 已定，可以照着实施。四节设计逐节确认过（2026-09-02）。
**起因：** 2026-09-02 用户问「我现在在 dc-llm 上有了自己的 key，怎么来配置 dc-terminal？」，
答案是「进界面按 `c`，粘贴，回车」之后，用户说了这句真正的需求：
**「使用 dct 的人大部分是小白」**，接着是**「我的目的就是直接在 dct 里面使用 dc-llm 的服务」**。
所以这份设计要解决的不是「怎么填一把钥匙」，是**学生装完 dct 就该能跑起来**。
**跨仓：** 这份设计同时管两个仓库——`dc-terminal`（dct）和 `dc_llm/admin-proxy`（网关）。
实现分两个仓提交，接口契约以本文为准。
**关联：**
- `profiles/dc.toml` / `profiles/qwen.toml` —— 08ec3f5 把两个 profile 指向了训练营网关，
  这份设计接着那一步走：那次解决了「地址对不对」，这次解决「钥匙怎么进来」。
- `2026-05-25-routing-transparency-design.md`（dc_llm 仓）—— 网关路由的权威说明。

---

## 问题

今天一个学生要在 dct 里用上训练营的算力，得走完这一串：

1. 知道 `dc-llm.tzspace.cn` 这个地址
2. 在网页上注册、登录
3. 找到个人空间里那个「密钥」
4. 认出那一串 `sk-...` 是要复制的东西
5. 切回终端，进 dct，按 `c`，选 DC，粘贴
6. 如果他还要用 qwen，把同一把钥匙再填一遍

六步里有五步跟他想做的事（让 AI 帮他写代码）没有任何关系。而这些人**大部分是小白**——
第 4 步「认出这一串是密钥」对他们就不是显然的，第 6 步「同一把钥匙填两个地方」更不是。

更糟的是第 5 步之后还不算完：dct 自己那个「报错看不懂时让 AI 解释」的功能
（`~/.dct/config.toml` 的 `[llm]`）默认整个关着，而且**故意**关着（见 `config.rs:6`
那段注释：这是隐私边界，不是懒得写默认值）。所以哪怕钥匙填对了，学生也不会
自动获得 dct 本身的 AI 能力——那需要他去编辑一个 TOML 文件，写四行他没见过的东西。

**目标：** 学生选中 `DC`，剩下的事 dct 自己办完。

**不做的：** 不做 dct 内建的手机号/验证码登录屏（理由见下一节）。不做把钥匙
预置进安装命令。不动 `profiles/` 里那两个仓库文件。

---

## 选定的方案：设备码流

照 OAuth device authorization grant 的形状做，因为这个形状被审过无数遍，
错误码和退避规则是现成的：

```
dct  → POST /admin/api/pair/start        （不带认证）
     ← {device_code, user_code:"HJ4K-9QTZ", verify_url, interval:2, expires_in:300}

dct  开浏览器到 verify_url，同时把 user_code 用大字印在终端上

学生 在网页里用现有方式登录（手机号+验证码，网关已有）
     → 页面显示「dct 想拿走你的密钥」+ 那串码 → 点确认
     → POST /admin/api/pair/approve {user_code}    （cookie 认证，现有那套）

dct  → POST /admin/api/pair/poll {device_code}     每 2 秒
     ← {status:"pending"} … 直到 ← {status:"approved", api_key, models, quota}
       （models 是按 wire 分组的**列表**，不是一个名字——见第 1 节）
```

**为什么是这条，不是别的两条：**

*dct 内建手机号+验证码屏*——学生连浏览器都不用开，看起来更省事。但 dct 就得亲手
拿手机号、验证码、session cookie 和 CSRF token，认证 UI 在网页和终端各写一份，
网关哪天改登录方式（加个图形验证码、换成微信扫码）dct 就得跟着发版。为了省一次
开浏览器，把整个认证面搬进一个终端程序，不划算。

*回调到 localhost*——dct 起个本地端口，网页登录完重定向回 `127.0.0.1:PORT/cb?key=...`。
少一次轮询，但钥匙走 URL query 会进浏览器历史，Windows 上第一次还弹防火墙。
不值。

*复用现有的手机扫码配对*（`src/qr.rs` + `dct-srv`）——方向反了：那套是「把手机配给
dct」，这里是「把网关账号配给 dct」，信任模型不同，硬套还要绕一个能看见钥匙的
中转服务。不值。

设备码流的好处一句话：**dct 从头到尾不碰手机号、验证码、密码、cookie**。
它只拿到一把它本来就该拿到的钥匙。

---

## 第 1 节：网关接口与配对状态机

新文件 `admin-proxy/app/routes_pair.py`，`APIRouter(prefix="/admin/api/pair")`，
在 `main.py` 里 `include_router`。

### 新表 `PairRequest`

| 列 | 说明 |
|---|---|
| `device_code_hash` | 32 字节 CSPRNG 的 hash。**只存 hash**，跟 `ApiKey` 一个待遇 |
| `user_code` | 8 位人读码，形如 `HJ4K-9QTZ`。字母表去掉 `0 O 1 I L`——学生要照着屏幕念 |
| `status` | `pending` / `approved` / `denied` / `expired` / `claimed` |
| `user_id` | 批准时才填 |
| `created_at` / `expires_at` | 有效期 5 分钟 |
| `poll_count` | 轮询次数，用于限流 |
| `client_ip` / `client_ua` | `start` 时记下，确认页要显示（见下） |

`user_code` 要**唯一索引 + 冲突重试**：8 位、去掉易混字符的字母表，撞是迟早的事，
而撞上的后果是「批准了另一条流程」。过期的行在下一次 `/pair/start` 时顺手清掉，
不为这个加调度器。（这两条 2026-09-02 由 `dc-llm-01` 补，SQLite 上唯一索引要用
`create_index(..., unique=True)`，见本节末尾的迁移注意事项。）

### 三个接口：冻结的线上契约

**这一节是契约，不是示意。** 两个仓库分头实现，字段名和类型以这里为准，
两边都不许临场发挥（2026-09-02 与 `dc-llm-01` 约定）。

**整套接口挂在 `DC_ADMIN_PAIRING_ENABLED` 下，默认 `False`。** 关着的时候三个接口
一律 404（不是 403——不存在的功能就该像不存在）。样板是 `config.py:111` 的
`free_quota_on_v1`，区别是那个默认 `True`，这个必须默认 `False`：生产只有一台机器、
没有 staging、`dev` 上的代码会被下一次部署带上线（dc_llm 今天已经推了四次）。
上线之后这个开关还有第二个用处——上课上到一半能把配对关掉。

```
POST /admin/api/pair/start                              无认证
  → {"client": "dct", "version": "0.2.5"}
     User-Agent: dct/0.2.5 (macos; aarch64)
  ← 200 {"device_code": "<64 hex>",
         "user_code": "HJ4K-9QTZ",
         "verify_path": "/pair",
         "interval": 3,
         "expires_in": 900}
  ← 404  开关关着
  ← 429  同 IP 刷 start

POST /admin/api/pair/approve                            cookie + CSRF（现有 current_user）
  → {"user_code": "HJ4K-9QTZ", "decision": "approve" | "deny"}
  ← 204
  ← 400  未知 / 已过期 / 已经表过态的码
  ← 401  没登录        ← 403  CSRF 不对

POST /admin/api/pair/poll                               无认证，device_code 即凭据
  → {"device_code": "<64 hex>"}
  ← 200 {"status": "pending"}
  ← 200 {"status": "approved", "api_key": "...", "base_url": "...",
         "models": {...}, "quota": {...}}
  ← 200 {"status": "denied" | "expired" | "claimed"}
  ← 404  device_code 不认识
  ← 429  轮询过快
```

**`poll` 的生命周期状态一律走 200 + `status` 字段，不走 4xx。** 只有「这个请求本身有
问题」（不认识的码、太快、功能关着）才用状态码。理由是 dct 那侧：状态机读一个
`status` 枚举，比既解析状态码又解析错误体少一半出错的地方，而 `denied` / `expired`
是**正常的**流程终点，不是错误。

**网关返回的是 `verify_path`，不是完整 URL——origin 由 dct 自己拼。**
（2026-09-02 修正，起因是 `dc-llm-01` 在实现时发现的钓鱼面。）

原稿让网关返回整条 `verify_url`。但 `/pair/start` 是**无认证**的，而 dct 拿到那个字符串
就**直接开浏览器**。网关若从 `X-Forwarded-Host` 之类推导 origin，任何能打到这个接口的人
都能让 dct 打开一个他控制的页面——而那个页面要的正是学生的控制台登录。

所以 origin 只能来自 dct **本地已经知道**的那个值（`profiles/dc.toml` 里 `[api].base_url`
的同源地址，随仓库发布，不来自网络）。dct 拼 `<本地已知 origin><verify_path>?code=<user_code>`。
网关连一个可被影响的 origin 都不必持有，这条路径上没有任何东西可以被伪造成别的站点。

留 `verify_path` 而不是让 dct 把路径也写死：SPA 哪天改路由，网关改一个字符串就行，
不用等 dct 发版。**路径是配置，origin 是信任锚**，两者不能混。

**dct 发的 User-Agent**（确认页要显示它，所以这里定死）：

```
dct/<CARGO_PKG_VERSION> (<std::env::consts::OS>; <std::env::consts::ARCH>)
例：dct/0.2.5 (macos; aarch64)
```

只有版本、系统、架构。**不带主机名、不带用户名**——那一行是要显示在网页上给人看的，
它的用处是「这台设备是不是我」，不是「这台设备是谁」。

`models` 和 `quota` 的形状见下两节。

**节奏与时限，由 dct 侧定，网关的限流数照这个配：**

| 值 | 定值 | 为什么 |
|---|---|---|
| `expires_in` | **900 秒** | 原稿写 300 秒，不够。学生很可能是**在这条流程里第一次注册**——收短信、设密码、看确认页，五分钟能烧完，而码一过期他要从头再来一遍 |
| `interval` | **3 秒** | 15 分钟最多 300 次轮询。1 秒太吵，5 秒让确认后的等待肉眼可见 |
| 429 退避 | 翻倍，封顶 **30 秒** | dct 侧自己退，不指望网关教它 |
| 网关节流阈值 | 快于 **2 秒**就 429 | 比 `interval` 松一档，别让正常客户端的抖动撞上限流 |

### `models` 的形状：按 wire 分组的列表，不是一个名字

**这一条 2026-09-02 修正过**（来源：`dc-llm-01` 会话，已核实）。原稿写的是
`{"default", "small_fast", "openai"}` 三个固定名字，不成立，两个原因：

- Claude 是**付费限定**。免费账号拿不到，给它写一个 Anthropic 模型名等于配一条
  跑不通的路。
- 免费账号默认平台是 `["local", "cloud"]`（`config.py:129`），免费那条路上的模型是
  `qwen3.8:27b` 一类。

**列表只能有一个来源。** `routes_free_playground.py:59` 的 `my_models` 已经在算这件事：
`ModelPrice` × `entitlements.active_for` × `served_models(db)`。配对**必须复用同一段
代码**（抽成一个共享函数，两处都调），不许在 pair handler 里另拼一份。那段注释写了
不用 `served_models` 的后果：没启用 anthropic provider 时，一个 `claude-*` 请求
**不会被拒绝**，它会掉到路由最后一步的 ollama 上，而那台机器没听说过这个模型。
两份列表一旦漂移，症状就是网页 playground 提供一个 dct 用不了的模型（或者反过来），
而且**没有任何东西会响**。

按 wire 分组返回**这个账号当前能用的**，dct 各取所需：

```json
"models": {
  "anthropic": {"default": "...", "small_fast": "..."},   // 付费才非空
  "openai":    {"default": "qwen3.8:27b", "small_fast": "..."}
}
```

`anthropic` 为空时 dct 的行为写在第 3 节：`dc` profile 照样配钥匙，但不写模型名，
也不拿它当 `[llm]` 的 provider——那种账号该走 `qwen`。

### 四条安全约束

1. **`approve` 走现有的 `current_user`**（`auth.py:13`，cookie + CSRF）。不新增认证
   路径 = 不新增被攻破的面。
2. **`poll` 成功一次就把行置成 `claimed`**，领取和置位在同一个事务里。钥匙不能领第二次。
3. **`user_code` 只用来批准，绝不用来领钥匙。** 它短、会被念出口、可能被旁人看见；
   `device_code` 才是凭据，它只在 dct 的内存里待过。
4. **`start` 按 IP 限流，`poll` 按 `device_code` 限流**（快于 1.5 秒回 429）。
   `routes_register.py` 里那个 `_PW_FAILS` 进程内节流是现成样板。**今天成立**：
   `Dockerfile:30` 是 `hypercorn app.main:app --bind 0.0.0.0:8700`，没有 `--workers`，
   生产 compose 也没有 `deploy.replicas`——一个容器一个事件循环。但它离失效只有一个
   flag：加上 `--workers 2`，每个 worker 一份自己的计数器，限流**静悄悄地**失守
   （`consumer_gate.InFlight` 建立在同一个假设上）。所以这里要的是**启动时的断言或
   一行日志**，不是一句注释——加副本的那个人不会来读注释。

### 钥匙从哪来：直接读，**绝不 rotate**

明文特意加密存在 hash 旁边，为的就是今天这个场景——`routes_free_playground.py:234`
那段注释说得很清楚：「一把谁也读不回来的钥匙，就是一把没人能粘进 agent 的钥匙」。

但**不要调 `my_api_key` 那个接口**：它是 `Depends(current_user)` + `_require_consumer`，
cookie + CSRF，只给浏览器。配对在 `approve` 那一刻手里就有 session，直接调 `_key_row()`
和 `SettingKV` 解密那两步。

**409 那条路不许 rotate。这是 2026-09-02 修正的一处真危险**（来源：`dc-llm-01`，
已核实 `routes_free_playground.py:295`）。原稿写「账号没有可读明文就当场 rotate 一把」，
而 `rotate_my_api_key` 撤销的是**这个租户下每一把 active key**，不是「换一把新的」。
学生粘在别处的钥匙——他另一台机器上的 dct、一个脚本、一个应用——会跟着一起死，
而他做的事只是「在训练营网页上点了个确认」。

改成：409 时配对**失败**，屏上说「你这个账号的密钥读不回来了，去
`dc-llm.tzspace.cn/me` 点『重新生成』再来配对」，并且那个页面上必须写明**旧钥匙会
全部失效**。撤销是个该由人当面拍板的动作，不是配对流程的副作用。

409 什么时候发生：`SettingKV["playground_key:<uid>"]` 缺失或解不开。注册流程一定会写
（`routes_register.py:390`），所以只剩早于那套安排的老账号，很少。

**审计要单列一个 reason。** 现在 reveal 记的是 `api_key_revealed`；配对要用自己的
（`api_key_paired`），否则「钥匙交给了一台设备」和「学生在浏览器里看了一眼自己的
钥匙」在审计里长得一模一样——出事那天要分的就是这两者。

### 额度接口

现有 `/admin/api/me/quota` **只认 cookie**（`auth.py:13`），dct 手里只有 key，用不了。

**`/v1/*` 终结在 admin-proxy，不是 Ollama。** 原稿在这里判断错了：读到的是 GPU 机器上
那份 appliance Caddyfile；生产用的 `deploy/production/caddy/Caddyfile` 是
`handle /v1/* { reverse_proxy admin-proxy:8700 }`，`upstream.py:mount_proxy` 在
FastAPI 上挂一个 `/v1/{full_path:path}` 的 catch-all，先做 key 认证、租户归属、
限流、预算、内容过滤、审计，再转上游。

所以新接口有现成的窝，也有现成的先例：`routes_public.py::create_public_router` 里
`/v1/key`、`/v1/usage`、`/v1/credits` 三个都是 Bearer 认证的读接口，走同一个
`auth.resolve_api_key`。新增：

```
GET /v1/me/quota    Authorization: Bearer <api_key>
  ← 200 {"used_micro": int, "limit_micro": int | null,
         "period_end": "<ISO 8601>" | null, "plan": str,
         "window": {"<platform>": {"used_tokens": int,
                                   "limit_tokens": int,
                                   "resets_at": "<ISO 8601>"}}}
```

**`window` 是 2026-09-02 加的，而且它才是真正会拦住学生的那个限额。**
（来源：`dc-llm-01`。）钱包按钱算，`usage_window` 按 token 算——免费档每平台
5 小时 20 万 token，从首次使用起算。**一轮 Claude Code 的对话在「贵」之前先「长」**，
所以学生会在面板还写着「还剩 ¥3.21」的时候被拒绝，理由是「本时段额度已用完」。
那读起来就是 dct 的 bug。

dct 侧的规矩：**显示两者中更接近耗尽的那个**，并且把话说成人话
（「本时段额度约 20 分钟后恢复」而不是「window.resets_at」）。

`poll` 的 `approved` 里那个 `quota` 用**同一个形状**，值取 `free_quota.snapshot(db, user_id)`
——跟 playground 面板同一个来源，两处不许各算各的。

**分两期，因为 9 月 6 日有课**（今天 9 月 2 日）：

- **一期**（课前必须有）：`quota` 快照跟着 `poll` 回来，配对成功屏上显示一次。不需要新接口。
- **二期**：`GET /v1/me/quota`，给密钥页那个「本月还剩多少」的实时值。这条晚一周
  不影响任何人配对成功。

**必须注册在 `mount_proxy` 之前**（`main.py:107` 那行注释就是这个意思），否则
catch-all 会把这个路径吞掉转给上游。

为什么不复用那三个现成的（都看过了，都不是这个东西）：`/v1/usage` 是按天按模型的
历史 token 数，`/v1/credits` 是充值余额和流水。学生要看的「这个月免费额度还剩多少」
两个都不回答——那个数在 `consumer_gate.py` 的钱包里。新接口的活就是把它读出来。

### `/me` 页面的确认 UI

`console/src/views/` 下加一个配对确认页，`verify_url` 指向它。页面上要有：
学生自己输入或从 URL 带入的 `user_code`、一句「dct 想拿走你的密钥」、
确认和拒绝两个按钮。**拒绝必须是个真按钮**——没有拒绝的确认页教人闭眼点确认。

**页面要显示这是哪台设备，不能只显示那串码。** 第 1 节说了「谁登录谁都能批准任何一串
码」是设计如此——但那正好是设备码钓鱼的形状：攻击者起一条流程，把自己的码念给学生
（「你把这个念一下就算装好了」），学生一批准，攻击者那边轮询就把钥匙收走了。码要**手输**
挡掉一部分，但它不告诉学生**他在给谁授权**。

所以 `/pair/start` 时记下 IP 和 User-Agent，确认页上原样显示：
`请求来自 192.168.1.10 · dct/0.2.5 (macos; aarch64)`。这不能阻止钓鱼，但它给了学生
一样具体的、能看出不对劲的东西——而「你自己那台机器的地址」是他唯一有可能认得出的
证据。再加一条：同一 session 每分钟能批准的次数要有上限。

**SPA 路由有个坑，不处理这页会跳走**：`console/src/router/index.ts:79` 是
`if (auth.isConsumer && to.name !== 'my-playground') return { name: 'my-playground' }`
——消费者账号访问任何别的路由都会被钉回 `/me`。新路由名要加进这个条件。

同时**不要**把它放进 `PUBLIC`（`index.ts:60`）：这条流程的全部意义就是「先登录，
再确认」，它必须要求 session。另外 SPA 现在从 `/` 提供，运营端在 `/admin/*` 下。

---

## 第 2 节：dct 侧的屏、轮询、取消

### HTTP 在 daemon 里，不在 UI 里

三个新请求，接着 `VerifySecret` 的路子走（`proto.rs`）：

```rust
Request::PairStart  { profile: String }  → Response::Pair(PairStarted { user_code, verify_url, expires_in })
Request::PairPoll   { profile: String }  → Response::PairPoll(PairPoll)
Request::PairCancel { profile: String }  → Response::Ok
```

**`device_code` 只活在 daemon 里**，一次也不过 socket。`proto.rs:374` 那个手写的
`Debug` 已经为 `SetSecret` / `VerifySecret` 脱敏，新变体照办。UI 拿到的只有
那串给人看的 `user_code`。

### 新视图 `View::Pair`

| 阶段 | 屏上是什么 | 能按 |
|---|---|---|
| `Starting` | 「正在联系训练营……」 | `Esc` 取消 |
| `Waiting { user_code, deadline }` | 大字 `HJ4K-9QTZ` + 「浏览器已经打开，登录后核对这串码，点确认」+ 倒计时 | `Esc` 取消，`o` 重开浏览器，`p` 手动填 |
| `Failed(reason)` | 一句中文加一条出路 | `r` 重来，`p` 手动填，`Esc` 退 |

`Starting` 一成功就 `open_url(verify_url)`（`ui/mod.rs` 已有），**并且把 URL 也印在
屏上**。浏览器打不开是常态——SSH 进来的、WSL、没设默认浏览器——那时候屏幕上必须有
一个能手抄的地址，否则学生就卡死在这一屏。

### 轮询在 daemon 里跑，UI 只问结果

理由跟 `verify_rx` 那条一样但更硬：轮询要跑 5 分钟，而 UI 那条连接 5 秒就超时
（`client.rs:11`）。所以 daemon 起一个后台线程按 `interval` 打 `/pair/poll`，
把最后状态存在内存里；UI 每次主循环 tick 发一条 `PairPoll` 读这个状态，不阻塞。
结果的排空点跟 `verify_rx` 一样在 `term.draw` 之前（`ui/mod.rs:451`）。

### 取消要真取消

`Esc` 发 `PairCancel`，daemon 停线程、丢掉 `device_code`。不发的话，用户退出去了，
后台还在替他领钥匙——领到了写进 secrets，而他以为自己取消了。`phone.rs:442`
那条测试（「Esc 要能取消正在飞的验证」）就是这个 bug 的前科，配对要有同样的断言。

### 超时

`expires_in` 到点，daemon 自己停，状态转 `Expired`，UI 显示「这串码过期了，按 `r`
换一串」。**不要让它无限轮询**——`start` 是无认证接口，一个忘在那儿的 dct 就是一台
每 2 秒敲一次网关的机器。

### 入口

学生根本不该主动去找「配对」这个词：

- 看板上选 `DC` 起会话，没钥匙 → 不再弹粘贴框，直接进 `View::Pair`
- 密钥页（`c`）里 DC 那一行 → 回车进 `View::Pair`
- **手动粘贴那条路留着**，藏在 `View::Pair` 的一行提示里（「有密钥？按 `p` 手动填」）。
  老用户、离线课堂、网关配对坏掉的那天，都得有条退路

---

## 第 3 节：钥匙落到哪、`[llm]` 那个勾、模型和额度

### 领到一次，写三处

`poll` 回 `approved` 那一刻，daemon 做完这三件事：

```
1. SecretStore::set("dc",   api_key)
2. SecretStore::set("qwen", api_key)      ← 同一把钥匙，两个方言口
3. 勾上了才写 ~/.dct/config.toml 的 [llm]
```

**不假装 1 和 2 是原子的。** `secrets.rs:134` 那套「save 失败就回滚内存」是按单键
写的，两次 set 就是两次落盘。第二次失败就报「qwen 那半没配上，去密钥页按回车重试」，
**而不是回滚第一次**——回滚会把学生刚拿到的、网关那边已经标成 `claimed` 的钥匙扔掉，
那把钥匙他再也领不回来了。领取是一次性的，这是这里唯一重要的约束。

### `[llm]` 那个勾

配对确认屏上一个勾选框，**默认勾上**，文案要说清代价而不只是好处：

```
[×] 报错看不懂时让 AI 解释（会把终端上的报错原文发给训练营网关）
```

勾上写：

```toml
[llm]
provider = "dc"
model = "<models.anthropic.default，为空则退到 provider = \"qwen\" + models.openai.default>"
transport = "http"
```

`provider` 就是 profile 名（`resolve.rs:125` 的 `lookup(name)`），所以写 `"dc"` 直接
复用 `profiles/dc.toml` 的 `[api].base_url` 和 `[secret].env`，不新增任何 provider 类型。
**不写 `base_url`**——留空就走 profile 里那个，一个地址一处维护。

`model` 是必填：`resolve.rs:164` 明确拒绝替用户猜（猜了在非 Anthropic 端点上稳定换 404）。
所以「模型名自动填」不是锦上添花，是这条路能不能通的前提。

**`config.rs:6` 那段注释要改。** 它现在说「没写 `[llm]` 就是关着，这是隐私边界不是
默认值」，改成说清楚现在多了一条「学生在配对屏上当面勾过」的路径。那段注释是这条
边界的唯一说明书，让它继续说一句已经不成立的话，比没有注释更糟。

### 模型名写哪儿

`profiles/` 是**仓库里的文件**，运行时不能写。走 `~/.dct/profiles/dc.toml` 覆盖层
（`profile.rs:378` 的 `all_profiles` 已经在做用户目录覆盖仓库的合并），写进那份的
`[env]`：

- `~/.dct/profiles/dc.toml` 的 `ANTHROPIC_MODEL` = `models.anthropic.default`
- 同一份的 `ANTHROPIC_SMALL_FAST_MODEL` = `models.anthropic.small_fast`
- `~/.dct/profiles/qwen.toml` 的 `OPENAI_MODEL` = `models.openai.default`

两个 Anthropic 变量**都要写**。`dc.toml` 里那段注释已经把理由写死了：claude 那个 CLI
干活用一个模型，起标题、扫文件这类杂活另外叫一个便宜的快模型，只钉住前一个的话，
杂活会以课堂上没人查得出来的方式坏掉。

**`models.anthropic` 为空怎么办**（免费账号的常态，Claude 是付费限定）：钥匙照写给
`dc`，但**不写模型名，也不把 `dc` 当 `[llm]` 的 provider**——那种账号该用 `qwen`，
`[llm]` 就写 `provider = "qwen"`、`model = models.openai.default`。配对成功屏上多一句
「你的账号现在走通义千问那条；Claude 需要付费开通」。

写一个跑不通的模型名比不写更坏：学生选了 `DC`，会话起来了，第一句话换回一个 404，
而屏幕上没有任何东西指向「你的账号没开这个」。

仓库里那两份 profile 保持现在的注释状态，一行不动。

### 额度

`poll` 的 `approved` 里带的 `quota` 是**配对当时那一刻的快照**，只用来在配对成功屏上
说一句「你这个月有 ¥10 额度」。之后每次要看当前值，走第 1 节新增的 `GET /v1/me/quota`。

密钥页 DC 那一行后面挂一句
「本月还剩 ¥3.21 / ¥10」，**进密钥页时拉一次，不轮询**。

看板上不显示：那是每秒重画的地方，往上面挂网络请求会把「dct 从不卡」这条性质毁掉。

拉不到就什么都不显示，**不显示错误**——额度是锦上添花，它挂了不该让学生以为
自己的钥匙坏了。

---

## 第 4 节：测试、失败路径、上线顺序

### 传输层注入，测试不打网络

`verify.rs:44` 已经立了规矩：判定逻辑吃一个 `send` 闭包，真传输在旁边。配对照抄：
`pair::poll_with(state, &dyn Fn(...) -> Result<PollBody, String>)`。于是下面全是
纯函数测试：

| 情形 | 期望 |
|---|---|
| 一直 `pending` 到 `expires_in` | 转 `Expired`，停轮询，**不再发请求** |
| `denied` | 立刻停，屏上说「你在网页上点了拒绝」 |
| 429 | 退避到 `interval × 2`，不放弃 |
| `approved` 但 `api_key` 是空串 | 当失败处理，绝不写一个空钥匙进 secrets |
| `approved` 但 `models` 两个 wire 都空 | 钥匙照写，`[llm]` 不写——`resolve()` 没 model 会拒绝，宁可不开也不写一份跑不起来的配置 |
| `approved` 但只有 `models.openai`（免费账号常态） | 钥匙两个 profile 都写，`dc` 不写模型名，`[llm]` 用 `provider = "qwen"` |
| 网络断 | 不算失败，接着轮询到过期 |
| `approved` 收到两次 | 第二次忽略，不重复写 secrets |

### 网关侧 pytest

底子现成（`tests/conftest.py`：每个测试一个 sqlite + `TestClient` + 种好的 cookie）。
每条对着第 1 节那四条约束：

- `approve` 不带 cookie → 401；带 cookie 不带 CSRF → 403
- 别人的 `user_code` 能不能被批准 → **能**，这是设计如此（谁登录谁批准，码就是给念的）；
  但 `poll` 只把钥匙给拿着 `device_code` 的那一方
- `poll` 领过一次 → 第二次 400 `claimed`
- 过期的 `user_code` 去 `approve` → 400
- `start` 同 IP 刷 → 429
- 表里存的是 `device_code` 的 hash 不是明文（直接断言列的内容）

### 网关侧的迁移有个坑

基线迁移是 `Base.metadata.create_all()`
（`alembic/versions/20260418_1235_44b113166eb6_baseline.py:27`），所以**全新装出来的库
已经有当前模型声明的每一张表**。新 revision 里不加守卫的 `create_table` 会在全新安装上
直接炸「table already exists」。抄
`20260824_1200_f6g7h8i9j0k1_consumer_signup.py` 里的 `_has_table` / `_has_column` /
`_has_index` 守卫（2026-09-02 那两个修复 `d9b33dd`、`e1ac8b7` 就是踩这个踩出来的）。

另外默认存储是 SQLite（`config.py:9`），Postgres 只在生产：

- 不许 `op.create_unique_constraint`（SQLite 不能往已有表上 ALTER 约束）——用
  `create_index(..., unique=True)`
- 不许不带方言判断的 `op.alter_column` 改类型

### dct 集成测试

`tests/pair_flow.rs`：`tests/common` 里加个最小 HTTP responder 当假网关，走完
start → pending → approved 整条，断言 `secrets.toml` 里两个键都在、`config.toml`
的 `[llm]` 按勾选与否出现或不出现。

再加一条 `Esc` 取消——`phone.rs:442` 那条断言的配对版。**这条必须有**，第 2 节
说过它有前科。

### 失败路径的出路

每一条都要在屏上给一个能按的键。没有出路的错误屏等于死路：

| 学生遇到 | 屏上说什么 | 能按什么 |
|---|---|---|
| 浏览器没打开 | 地址原样印出来 | 手抄，或 `o` 重试 |
| 码过期了 | 「这串码过期了」 | `r` 换一串 |
| 网关连不上 | 「联系不上训练营，检查网络」 | `r` 重来，`p` 手动填 |
| 账号没注册 | 网页那边引导注册（网关现有 `register` 流程），dct 这边照常等 | 什么都不用做 |
| 配对整个坏了 | 那行提示一直在 | `p` 手动填 |

### 上线顺序

网关先走，dct 后走——反过来会有一版 dct 带着一条注定 404 的路出门。

1. 网关：三个接口 + 表 + `/me` 确认页 + `/v1/me/quota`，上 QA，pytest 绿
2. dct 指着 QA 手工跑一遍全程（这一步没法自动化：要真开浏览器、真点确认）
3. dct 合入，`profiles/` 里两个文件不动
4. 网关上生产，dct 发版

### 一条让这件事今天才安全的前提

消费者的 key 在 `/v1` 上**现在是计量的**（`app/consumer_gate.py`：日钱包、5 小时
token 窗口、平台准入、每账号一个在飞请求）。在那之前，把一把 key 发出去等于绕过
所有额度上限。**配对流程之所以现在能做，是因为这一层已经在了**——它哪天被摘掉或绕过，
这份设计的风险评估就要重做。

### 撤销义务

配对领出去的每一把钥匙都是可撤的：`POST /admin/api/me/api-key/rotate`。学生钥匙泄漏
（贴群里、提交进作业仓库）的处置就是这一条。`/me` 页面上要有这个按钮，dct 的密钥页里
也要提一句它在哪。

**按钮旁边必须写清它的范围**：这个接口撤销的是该租户下**每一把** active key
（`routes_free_playground.py:295`），不是「换掉当前这一把」。学生别处配好的 dct、
脚本、应用会一起停。这正是配对流程自己绝不许悄悄调它的原因（见第 1 节）。

---

## 谁写哪半

2026-09-02 与 `dc-llm-01` 会话约定，**按仓库切，不按功能切**。理由是雷都在 dc_llm 那边
而它正站在上面：今天它修了三个迁移、落了 `/v1` 计量和密钥读取、往生产推了四次。
这周 `admin-proxy/` 有第二双手，就是 alembic 两个头加生产上一个半开的接口。

**dc_llm 那边（`dc-llm-01` 写）**：三个接口、设备码表的迁移（带全新安装守卫）、
`poll` 的响应组装、console 里的确认页和路由守卫改动、`/me` 那个「连接你的工具」
版块的 DC-TERMINAL 分支、二期的 `/v1/me/quota`。迁移从 `p7q8r9s0t1u2` 分出去。

**dc-terminal 这边（本仓库写）**：这份 spec 以及线上契约的最终解释权、dct 客户端
（start / 开浏览器 / 轮询 / 退避 / 超时）、`View::Pair` 那几屏和失败文案、
钥匙写入 `secrets.toml` 并**用一次配对填两个 profile 各自的模型名**、`[llm]` 那个勾。

**四条不许越界的规矩：**

1. `admin-proxy/` 和 `console/` 只有 dc_llm 那边提交。这边要改，发消息过去。
2. 开关打开、真在生产上服务之前，dct 这侧必须先对着它跑通过一次。
3. 这边合并期间 dc_llm 那边挂起部署；不在合并期就照常从 `dev` 推。
4. 迁移只有一个头：这边一个都不加。

## 顺带发现，不在本设计范围内

`README.zh-CN.md:264` 还写着「DC 没有申领页面：它的密钥由疯狂AI训练营发给上课的人」。
08ec3f5 已经把 profile 改成学生自己在 `/me` 拿，这份设计更进一步。README 那段
等实现落地后要重写，但那是另一个提交。
