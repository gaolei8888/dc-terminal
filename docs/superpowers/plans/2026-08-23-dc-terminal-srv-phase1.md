# dc-terminal-srv 第一期：打通链路 —— 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 手机上打开一个网页，用 dc_classroom 账号登录，看到自己那台电脑上的会话列表，
点进去看到**实时画面**。只读，不加密。

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

### 任务 1：拆 workspace，建 `dct-link`

- [ ] 根 `Cargo.toml` 加 `[workspace] members = [".", "crates/dct-link", "crates/dct-srv"]`
- [ ] 新建 `crates/dct-link`，只放 `Envelope`、`EndpointId`、`AuthFrame`、错误码、`LINK_VERSION`
- [ ] `dct` 依赖 `dct-link`；**`src/proto.rs` 一行不动**
- [ ] 测试：信封 JSON 形状被钉住（同 `proto.rs` 里 `the_request_shape_is_pinned_to_the_protocol_version` 的写法）
- [ ] 验收：`cargo tree -p dct` 里没有 `tokio`；`cargo test --workspace` 全绿

### 任务 2：srv 骨架 —— 路由与设备表

- [ ] `crates/dct-srv`：`tokio` + `axum`，`POST /link/poll`、`POST /link/send`
- [ ] 内存里的设备表：`device_id -> 一个等待中的长轮询`（先不接 dc_classroom）
- [ ] 路由规则：信封按 `to` 投递；对方不在线就立刻回「设备不在线」，**不排队、不落盘**
      （spec「不做什么」第一条）
- [ ] 测试：两个假端点互发，A→B 到得了；B 不在线时 A 拿到明确错误
- [ ] 变异测试：把「不在线就报错」改成「静默丢弃」，必须有测试挂

### 任务 3：daemon 出网（`src/link.rs`）

- [ ] 独立线程，`ureq` 长轮询 `POST /link/poll`（超时 30s，空转即重发）
- [ ] 收到信封 → 解出 `Request` → **走 `daemon.rs` 现有的 dispatch** → 回复包成信封 POST 回去
- [ ] 断线：指数退避重连；心跳 45 秒（短于运营商 NAT 回收，spec「断线」）
- [ ] 线程 panic 不许拖垮守护进程（`catch_unwind`，同 `bridge.rs::spawn`）
- [ ] 测试：本机起一个假 srv（`127.0.0.1:0`），跑通一次 `List` 往返；断开后能自己重连
- [ ] 验收：**tick 线程里没有任何网络调用**

### 任务 4：手机网页 —— 会话列表

- [ ] `crates/dct-srv/web/index.html`：单页、自包含、深浅色跟随系统、中英双语
      （做法同 `site/index.html`）
- [ ] 拉 `List`，渲染成卡片：项目名、会话名、状态、最后一行 activity
- [ ] 空态、断线态都要有话说（「你的电脑睡着了」——spec「agent 跑在哪里」第 1 条）
- [ ] 测试：给定一份 `Response::List` 的 JSON，渲染函数产出的 DOM 里有那几个会话名

### 任务 5：手机网页 —— 实时画面（只读）

- [ ] 点进一个会话 → 300ms 拉一次 `Screen`，**只在页面前台时拉**（`visibilitychange`）
- [ ] 把 `ScreenSpan` 渲染成 HTML（结构化，不是图片），等宽字体，可缩放平移
- [ ] **不发 `Resize`**（spec 决定四）。画面按桌面宽度渲染，窄屏靠缩放
- [ ] 测试：一屏带颜色的 spans 渲染之后，颜色和文字都在；锁屏（`visibilitychange`）之后不再发请求

### 任务 6：接 dc_classroom 鉴权

- [ ] `Accounts` trait：`fn who(&self, token: &str) -> Result<UserId>`
- [ ] 真实现调 dc_classroom REST；测试实现是内存表
- [ ] 设备属于账号：`/link/*` 每个请求都验，跨账号投递直接拒
- [ ] 测试：A 账号的手机拿不到 B 账号设备的任何东西——**这条要写成安全回归测试**
- [ ] 变异测试：把账号校验去掉，上面那条必须挂

### 任务 7：配对（第一期简版）

- [ ] dct 设置页「手机」：显示一个 6 位配对码 + srv 地址；token 存 `~/.dct/secrets.toml`
- [ ] 手机上输码绑定；设备列表能看到名字（主机名）和最后在线时间，能撤销
- [ ] 被撤销的 daemon 下一次心跳收到拒绝 → 本地 token 作废，设置页说明白
- [ ] **二维码和 X25519 是第二期**，这里只做够用的最小版本
- [ ] 测试：撤销之后 daemon 的下一次 poll 拿到拒绝，且不再重试

### 任务 8：配额与收尾

- [ ] 按账号限：设备数、并发连接、每日字节
- [ ] 超额给明确的话（「今天的额度用完了」），不静默断开
- [ ] srv 只监听内网地址（第一期的硬性验收条件）
- [ ] README 里加一节说明这个功能存在、以及它现在还没加密

---

## 第一期不做

- 加密、二维码、X25519（第二期）
- 手机上打字、虚拟键行、输入法、滚回历史（第三期）
- 差量传屏（第三期；第一期整屏传，先把链路证明出来）
- 推送（第四期）
- 老师看学生屏幕（要多收件人信封，第二期之后）
