# dc-terminal-srv 第一期：打通链路 —— 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**前置：先做第 0 期。** `2026-08-23-dct-phone-lan-phase0.md` 在局域网里把手机端整个做完
（网页、渲染、输入、虚拟键行、输入法），**不需要服务器**。那一期做完之后，本期只剩换一根管子。
**顺序理由**：手机端未知数最多的活跟传输无关，放在能秒级迭代的环境里做快得多；而且第 0 期
会回答「你到底会不会用它」——答案是「基本不开」的话，本期就不该做。

**Goal:** 把第 0 期那个手机端从「同一个 WiFi」拓宽到「任何网络」：用 dc_classroom 账号登录，
经中转看到自己那台电脑上的会话。**手机网页一行不改。**

**Architecture:** 笔记本上的 daemon 主动出网，向 srv 长轮询取「有没有人要问你什么」，
把答案 POST 回去；手机网页向 srv 发同样形状的请求。**srv 只按设备 id 路由信封，不解析
`Request`/`Response`**（spec 决定一）。

**Tech Stack:** dct 侧 Rust ≥ 1.80、`ureq`（已有依赖，阻塞式 HTTP），**不引入 async**；
srv 侧独立 crate，`tokio` + `axum`。

**Spec:** `docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md`

---

## 两处跟 spec 措辞不同的地方（先读这一段）

**一、共享 crate 只装信封，不装 `Request`/`Response`。** spec 的「仓库布局」写的是
`dct-proto` 里放 `Request/Response/PROTOCOL_VERSION`，但**决定一说 srv 不解析它们**——
既然不解析，srv 就不需要这些类型。共享的只有信封和鉴权帧，`proto.rs` 原地不动。

后果是这次改动**几乎不动现有代码**：根目录仍然是 `dct` 这个包，只是多一个
`[workspace]` 段和两个新 crate。不用把 `src/` 整个搬进 `crates/dct/`——那会产生一个
几百个文件的 rename diff，把这次真正的改动淹掉。

**二、srv 用 async，dct 不用。** 「不引入 async 运行时」这条约束是给守护进程和 TUI 写的
（`2026-08-10-dct-phone-channel.md`），把它照搬到中转服务上是刻舟求剑：中转服务的全部工作
就是**同时挂着几千条空闲长连接**，这正是 async 唯一真正不可替代的场景。一个教室 1000 台设备
用线程模型就是 1000 条常驻线程。

srv 是独立 crate，`tokio` 不会进 `dct` 的依赖树——`cargo tree -p dct` 里看不到它。

**三、第一期不加密，所以第一期不许开放给真实用户。** 只在局域网和内部账号上跑。
加密是第二期，必须在任何一个真实学员用上之前落地（spec 决定二）。这条不是流程建议，
是这份计划的验收条件之一：第一期上线 = srv 只监听内网地址。

---

## Global Constraints

- **这份计划里的参考代码不是权威。** 它是意图的说明，不是可以照抄的成品。照抄之前先想它
  对不对；测试通过之后做变异测试。
- **变异测试是每个任务的收尾动作**：把实现里的一个判断取反、一个边界 ±1、一个 `&&` 改成
  `||`，跑测试。**没有测试失败 = 测试没写够，回去补。**
- **daemon 的 200ms tick 线程绝不做网络 IO。** 出网这件事在自己的线程里做，跟 `bridge.rs`
  同一条规矩——那个线程卡住不能拖累任何会话。
- **srv 永不解析 payload。** 任何一处 `serde_json::from_slice::<Request>` 出现在 `dct-srv`
  里都是这次设计被破坏的信号。
- 网页上每一句话都写给没编过程序的人；错误信息不给出下一步就是没写完。
- 不用 emoji 当图标。
- 界面文案中英双语。
- 测试不碰公网。绑 `127.0.0.1:0` 起真服务是允许的（仓库已有先例）。
- 每个任务结束前跑：`cargo test --workspace -- --test-threads=1`、`cargo fmt --check`、
  `cargo clippy --all-targets`。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `Cargo.toml`（改） | 加 `[workspace]`，成员为根包 + 两个新 crate |
| `crates/dct-link/`（新建） | **共享**：信封、鉴权帧、错误码、常量。不含 `Request`/`Response` |
| `crates/dct-srv/`（新建） | 中转服务：路由、鉴权、配额、静态网页 |
| `crates/dct-srv/web/`（新建） | 手机网页（单页，自包含，同 `site/index.html` 的做法） |
| `src/link.rs`（新建） | daemon 侧的出网客户端：长轮询、重连、把信封交给现有 dispatch |
| `src/daemon.rs`（改） | 起 link 线程；信封里的 `Request` 走**现有**分发，不另开一条路 |
| `src/proto.rs`（改） | 只加 `Request::Link*`（配对/状态），`Request`/`Response` 本体不动 |
| `src/ui/phone.rs`（改） | 设置页里显示配对状态、配对码、已连设备 |
| `src/i18n.rs`（改） | 新增文案 Key |

---

## 信封（`dct-link`）

```rust
/// srv 看得懂的全部东西。`payload` 对它是不透明字节。
pub struct Envelope {
    /// 谁发的：设备 id 或者 "phone:<会话 id>"
    pub from: EndpointId,
    /// 发给谁
    pub to: EndpointId,
    /// 请求/响应配对用；同一个 from 上单调递增
    pub seq: u64,
    /// 第一期是明文 JSON，第二期换成密文。**srv 两期都不看它。**
    pub payload: Vec<u8>,
    /// 第二期用：这份 payload 加密给了谁。第一期恒空。
    /// 现在就留位置，免得将来「让老师也能看」只能靠把加密关掉。
    pub recipients: Vec<PubKey>,
}
```

**`recipients` 第一期就要在结构里**，哪怕恒为空——理由见 spec 决定二末尾。

---

## Tasks

### 任务 1：拆 workspace，建 `dct-link` ✅（2026-08-28）

- [x] 根 `Cargo.toml` 加 `[workspace]`。**`members` 里只有 `crates/dct-link`**：
      计划原文写的是 `[".", "crates/dct-link", "crates/dct-srv"]`，但列一个还不存在的
      成员 cargo 直接报错，`dct-srv` 等任务 2 建出来再加；根包是隐式成员，`"."` 不用写
- [x] 新建 `crates/dct-link`，只放 `Envelope`、`EndpointId`、`AuthFrame`、`LinkError`、
      `EndpointKind`、`PubKey`、`LINK_VERSION`、`MAX_PAYLOAD`
- [x] `dct` 依赖 `dct-link`；**`src/proto.rs` 一行不动**
- [x] 测试：信封、鉴权帧、错误码三样的 JSON 形状都钉在 `LINK_VERSION` 上
- [x] 验收：`cargo tree -p dct` 里没有 `tokio`；`cargo test --workspace` 全绿（1023 + 8）

**计划里没写、做的时候才发现要定的三件事：**

1. **payload 走 base64，不走 serde 默认的数字数组。** 默认写法一个字节要四五个字符，
   整屏画面上公网时这个放大倍数是要命的；base64 只放大 1.33 倍。为此给 `dct-link`
   加了 `base64` 依赖（纯 Rust，不破坏「整棵依赖树一行 C 都没有」）。
2. **`EndpointId` 是个校验过的新类型，不是 `String`。** 它是中转的路由键，从网络上来：
   不限长等于让人把设备表撑爆，不限字符集等于放控制字符和换行进来——今天当 HashMap
   的 key 没事，明天有人把它写进日志或 HTTP 头就出事。校验挂在 `TryFrom<String>` 上，
   所以**走 serde 解出来的 id 也必须过同一道检查**（有测试盯着这条）。
3. **`AuthFrame` 多了 `kind`（`Computer` / `Phone`）。** 笔记本带的是配对 token，手机带的
   是 dc_classroom 的登录 token，验法完全不同；不写这个字段，中转只能拿凭据挨个验证器
   去猜。

变异测试：长度边界 `>` 改 `>=`、删空串检查、字符集判断取反、删超大 payload 预检、
去掉 `try_from` 让 JSON 绕过校验、去掉 `recipients` 的 `serde(default)`——六个变异全部
有测试挂。

### 任务 2：srv 骨架 —— 路由与设备表 ✅（2026-08-28）

- [x] `crates/dct-srv`：`tokio` + `axum`，`POST /link/poll`、`POST /link/send`
- [x] 内存里的设备表。**不是**「`device_id -> 一个等待中的长轮询`」，见下面第一条
- [x] 路由规则：信封按 `to` 投递；对方不在线就立刻回 `Offline`，**不排队、不落盘**
- [x] 测试：13 条（10 条在 `lib.rs`，3 条真绑端口走 HTTP）
- [x] 变异测试：8 个变异全部有测试挂，包括计划点名的那一条

**做的时候发现计划有一处是错的，改了：**

1. **设备表不能是「id → 一个等待中的长轮询」。** 那个模型里「在线」等于「此刻正
   挂着一个轮询」，可守护进程收到信封、处理完、再发起下一次轮询，中间有一条谁都
   不挂着的缝。手机的请求落在缝里就会被告知「你的电脑离线了」，而那台电脑好好的。
   改成：**在线 = 最近 `presence_ttl` 内轮询过**，每台设备挂一个有界信箱（channel），
   落在缝里的信封进信箱，下一次轮询立刻取走。`presence_ttl = poll_timeout * 3`，
   必须大于一次轮询的时长，否则正挂着的轮询会把自己熬成离线（有测试盯着）。

   这不违反「不排队、不落盘」：那条说的是**不给离线的人存东西**。信箱只在设备
   还活着时存在，有界，满了就明说，进程一停就没了。

2. **`must_be_loopback` 提前到本任务。** 计划把「srv 只监听内网地址」放在任务 7，
   但那是几天之后的事，而现在 token 根本没人验（任务 5）、内容也没加密（第二期）：
   这中间任何一次手滑把它绑到 `0.0.0.0`，就是一个谁都能冒充谁的中转挂在公网上。
   判断写成了库里一个有测试的函数，不是 `main` 里一句注释。任务 7 那条勾照旧要打，
   到时候是把它换成一个要人动手打开的开关。

3. **多了 `LinkError::Busy`**（信箱满）。跟 `Offline` 分开：一个该说「你的电脑离线
   了」，一个该说「你的电脑忙不过来」，两句话指向完全不同的排查方向。

4. **`send` 会比对 `envelope.from` 和 `auth.endpoint`。** 收件方判断「这是谁说的」
   只有 `from` 一个依据，不比这一下，任何连得上中转的人都能冒充别人。计划没写这条。

变异测试：静默丢弃代替报错、不查在线是否过期、不比对寄件人、不查版本、信箱满了谎报
离线、`presence_ttl` 短于一次轮询、坏消息回 200、取消只许绑环回——八个全挂。

### 任务 3：daemon 出网（`src/link.rs`）

- [ ] 独立线程，`ureq` 长轮询 `POST /link/poll`（超时 30s，空转即重发）
- [ ] 收到信封 → 解出 `Request` → **走 `daemon.rs` 现有的 dispatch** → 回复包成信封 POST 回去
- [ ] 断线：指数退避重连；心跳 45 秒（短于运营商 NAT 回收，spec「断线」）
- [ ] 线程 panic 不许拖垮守护进程（`catch_unwind`，同 `bridge.rs::spawn`）
- [ ] 测试：本机起一个假 srv（`127.0.0.1:0`），跑通一次 `List` 往返；断开后能自己重连
- [ ] 验收：**tick 线程里没有任何网络调用**

### 任务 4：手机网页 —— 换一个数据来源

**页面本身来自第 0 期，不重写。** 它现在是从 daemon 直接拉 `/api/sessions` 和
`/api/screen`；本期只把那两个 URL 换成 srv 的地址，并在前面加一次登录。

- [ ] 把第 0 期的 `src/web/page.html` 挪成 srv 的静态资源，**改动只限于取数的那几行**
- [ ] 登录：dc_classroom 账号 → 设备列表 → 选一台
- [ ] 断线态多一种说法：「你的电脑离线了」跟「你的电脑睡着了」要分得开
- [ ] 测试：页面渲染函数的测试从第 0 期原样搬过来，必须继续绿——**它是没重写的证据**

### 任务 5：接 dc_classroom 鉴权

- [ ] `Accounts` trait：`fn who(&self, token: &str) -> Result<UserId>`
- [ ] 真实现调 dc_classroom REST；测试实现是内存表
- [ ] 设备属于账号：`/link/*` 每个请求都验，跨账号投递直接拒
- [ ] 测试：A 账号的手机拿不到 B 账号设备的任何东西——**这条要写成安全回归测试**
- [ ] 变异测试：把账号校验去掉，上面那条必须挂

### 任务 6：配对（第一期简版）

- [ ] dct 设置页「手机」：显示一个 6 位配对码 + srv 地址；token 存 `~/.dct/secrets.toml`
- [ ] 手机上输码绑定；设备列表能看到名字（主机名）和最后在线时间，能撤销
- [ ] 被撤销的 daemon 下一次心跳收到拒绝 → 本地 token 作废，设置页说明白
- [ ] **二维码和 X25519 是第二期**，这里只做够用的最小版本
- [ ] 测试：撤销之后 daemon 的下一次 poll 拿到拒绝，且不再重试

### 任务 7：配额与收尾

- [ ] 按账号限：设备数、并发连接、每日字节
- [ ] 超额给明确的话（「今天的额度用完了」），不静默断开
- [ ] srv 只监听内网地址（第一期的硬性验收条件）
- [ ] README 里加一节说明这个功能存在、以及它现在还没加密

---

## 第一期不做

- 加密、二维码、X25519（第二期）
- 手机上打字、虚拟键行、输入法、滚回历史（**第 0 期就做完了**，本期不碰）
- 差量传屏（第三期；第一期整屏传，先把链路证明出来）
- 推送（第四期）
- 老师看学生屏幕（要多收件人信封，第二期之后）
