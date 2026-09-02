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

### 三个接口

```
POST /admin/api/pair/start        无认证
  ← 200 {device_code, user_code, verify_url, interval: 2, expires_in: 300}

POST /admin/api/pair/approve      cookie 认证（现有 current_user）
  → {user_code, decision: "approve" | "deny"}
  ← 204
  只认 pending 且未过期的；user_code 比对前把大小写和连字符归一化

POST /admin/api/pair/poll         无认证，device_code 本身即凭据
  → {device_code}
  ← 200 {status: "pending"}
  ← 200 {status: "approved", api_key, base_url, models, quota}
  ← 400 {status: "denied" | "expired" | "claimed"}
  ← 429 轮询过快
```

`models` 的形状，三个字段各有各的去处：

```json
"models": {"default": "...", "small_fast": "...", "openai": "..."}
```

### 四条安全约束

1. **`approve` 走现有的 `current_user`**（`auth.py:13`，cookie + CSRF）。不新增认证
   路径 = 不新增被攻破的面。
2. **`poll` 成功一次就把行置成 `claimed`**，领取和置位在同一个事务里。钥匙不能领第二次。
3. **`user_code` 只用来批准，绝不用来领钥匙。** 它短、会被念出口、可能被旁人看见；
   `device_code` 才是凭据，它只在 dct 的内存里待过。
4. **`start` 按 IP 限流，`poll` 按 `device_code` 限流**（快于 1.5 秒回 429）。
   `routes_register.py` 里那个 `_PW_FAILS` 进程内节流是现成样板——同样的理由、
   同样的做法，包括「多副本部署时要搬去 Redis」那条注释。

### 钥匙从哪来

调现有 `_key_row` / `my_api_key` 那段逻辑（`routes_free_playground.py:234`）。
账号还没有可读明文就当场 rotate 一把。那段代码的注释写的就是这件事的理由：
「一把谁也读不回来的钥匙，就是一把没人能粘进 agent 的钥匙」——明文特意加密存在
hash 旁边，为的就是今天这个场景。

### 额度接口

现有 `/admin/api/me/quota` **只认 cookie**，dct 手里只有 key，用不了。新增：

```
GET /v1/me/quota    Authorization: Bearer <api_key>
  ← {used_micro, limit_micro, period_end, plan}
```

放在 `/v1/*` 下不是随便挑的：那一段本来就是 key 认证的地盘，`/admin/api/*` 是
cookie 的地盘。两套认证不混在同一个前缀下。

### `/me` 页面的确认 UI

`console/src/views/` 下加一个配对确认页，`verify_url` 指向它。页面上要有：
学生自己输入或从 URL 带入的 `user_code`、一句「dct 想拿走你的密钥」、
确认和拒绝两个按钮。**拒绝必须是个真按钮**——没有拒绝的确认页教人闭眼点确认。

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
model = "<poll 回来的 models.default>"
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

- `ANTHROPIC_MODEL` = `models.default`
- `ANTHROPIC_SMALL_FAST_MODEL` = `models.small_fast`
- `~/.dct/profiles/qwen.toml` 的 `OPENAI_MODEL` = `models.openai`

两个 Anthropic 变量**都要写**。`dc.toml` 里那段注释已经把理由写死了：claude 那个 CLI
干活用一个模型，起标题、扫文件这类杂活另外叫一个便宜的快模型，只钉住前一个的话，
杂活会以课堂上没人查得出来的方式坏掉。

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
| `approved` 但没有 `models.default` | 钥匙照写，`[llm]` 不写——`resolve()` 没 model 会拒绝，宁可不开也不写一份跑不起来的配置 |
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

### 撤销义务

配对领出去的每一把钥匙都是可撤的：`POST /admin/api/me/api-key/rotate` 作废旧的。
学生钥匙泄漏（贴群里、提交进作业仓库）的处置就是这一条。`/me` 页面上要有这个按钮，
dct 的密钥页里也要提一句它在哪。

---

## 顺带发现，不在本设计范围内

`README.zh-CN.md:264` 还写着「DC 没有申领页面：它的密钥由疯狂AI训练营发给上课的人」。
08ec3f5 已经把 profile 改成学生自己在 `/me` 拿，这份设计更进一步。README 那段
等实现落地后要重写，但那是另一个提交。
