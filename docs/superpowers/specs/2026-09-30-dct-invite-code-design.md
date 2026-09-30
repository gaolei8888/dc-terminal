# dct 多电脑：用 6 位邀请码加电脑 —— 设计

**状态：** 用户已审（2026-09-30），进入实施计划。第 1–3 段已在对话里逐段确认；「安全隐患」一节是写文档时补的，需要用户看一眼。
**起因：** 2026-09-30 用户第一次在真电脑上走加入流程，问「这个操作怎么这么复杂？能不能简化？」。现在要四步：两台都 `dct login`、新电脑 `dct join`、两块屏幕对 6 位数、老电脑 `dct peers approve`。
**用户的决定（2026-09-30）：**
- 选「一条命令 + 邀请码」：老电脑出码，新电脑 `dct join 482913`，不再回老电脑点同意。
- 先自己用，以后给学生用 —— 设计要让小白能用，也要经得住学生规模。
- 新电脑必须已配对同一个 DC 账号（`dct join` 自动登录）；「连 DC 账号都不用配」留到以后单独设计。
- 6 位数字，用 PAKE，不用长码。
- **删掉旧流程**（dct-sas-v2 核对 + approve），不留两套。
- 组是长期的，不改。

**关联：**
- `2026-09-28-dct-multi-machine-design.md`：多电脑第一步。本设计**替换**其中「加一台新电脑」「6 位数怎么算（dct-sas-v2）」两节，其余（身份、名单、留言、中转令牌附录）不变。

---

## 现状（2026-09-30 查代码）

- 加入靠 commit-reveal SAS：新电脑 B 向同账号在线电脑（≤16 台）发 `Join{member, sig, commit}`，老电脑 A 回 `JoinPending{member, nA}`，B 发 `JoinReveal{nB}`，两边各算 6 位码给人对（`crates/dct-mesh/src/sas.rs`、`src/mesh/mod.rs:446-525`、`src/mesh/group.rs:112-300`）。
- A 上人按同意 → `MeshApprove{endpoint, code}` → A 签名单 v+1 并广播；B 只收它确认过的那台发来的名单（`roster::accept_invite`，`crates/dct-mesh/src/roster.rs:370`）。
- `dct join` 要求已 `dct login`，`dct login` 要求已配对 DC 账号（`src/mesh/cli.rs:280`、`src/daemon.rs:402-466`）。
- 中转只做同账号路由（配了 `--relay-keys` 时）；不存状态，也不限速（`crates/dct-srv/src/lib.rs:284-324`）。
- 依赖里没有 PAKE 库；已有 p256、x25519-dalek 2、chacha20poly1305、hkdf、sha2。

---

## 第 1 段：用户看到的流程

**老电脑（已经在组里）**
- 看板上按 `a`（加电脑），或敲 `dct invite`：显示 `邀请码 482 913 · 10 分钟内有效`，带倒计时。
  - 不用 `i`：宫格视图里 `i` 是「开回复框」（`src/ui/grid.rs:252`），同一个键两个意思会按错。
- 码一次性：用过一次（成不成都算）、或者过期，就作废。
- 同一时间一台电脑只有一个有效码；再按一次换新码，旧的作废。
- 组里只有自己一台也能发邀请（第一台电脑就是这样拉第二台）。

**新电脑**
- `dct join 482913 [--name 公司电脑]`。输入里的空格、`-` 忽略。
- 没登录就先自动登录（要已配对同一个 DC 账号）；已经在别的组里（名单上不止自己）就拒绝，提示先 `dct peers remove` 或重置。
- 成功：`已加入「我的电脑」，组里有：Mac、公司电脑`。老电脑看板提示 `公司电脑 已加入`，码消失。

**出错**
- 码不对 / 已过期 / 已被用掉：`邀请码不对或已作废，请在老电脑上按 a 重新生成`。老电脑那边码同时作废，看板提示「有人用错码试过一次，码已作废」。
- 找不到发邀请的电脑：`没有在线的电脑发出邀请` + 提示检查两台是不是同一个 DC 账号、老电脑的 dct 开着没有。
- 同账号有两台以上同时发了邀请：逐台试；对不上的那台码也作废，提示用户。少见。
- 守护进程重启：码丢失，重新按 `a`。

**删掉的：** `dct peers approve`、`dct join --confirm`、看板顶部 y/n 确认行、`dct peers` 里的待批准列表。`dct peers`、`dct peers remove`、`dct send` 不变。

---

## 第 2 段：协议（dct-invite-v1）

参与方：A 发邀请，B 加入，R 中转。R 能看、改、扣、冒充同账号里的任何端点，只受「同账号」限制。

### A 的邀请状态

```
Invite { code: [u8; 6] 十进制数字, created, expires = created + 10 分钟,
         state: Live | InFlight { peer: Endpoint, pake, deadline } | Burned }
```

- 码用系统随机数（`getrandom`），拒绝采样得到均匀的 000000–999999。
- 只在内存里，不落盘。

### 三次 ask（全走现有 `/link/ask`，R 认证发件人 `from`）

1. **探问** B → 每台同账号在线端点（沿用 `MAX_JOIN_ASK`=16 上限、排序去重）：`InviteProbe{}`。
   - A 有 `Live` 码 → `InviteOpen{member: A 的成员记录, sig: A 自签}`；否则 → `NoInvite`。
   - 不消耗码。B 核对 A 的自签名、`member.endpoint == env.from`。
2. **加入** B → A：`InviteJoin{member: B, sig: B 自签, spake: msgB}`。
   - A 检查：码是 `Live`；B 自签有效且 `member.endpoint == env.from`；B 不在名单；名字合法。
   - 任何一项不过 → 回 `InviteFailed`，**码是否作废见下**。
   - 全过 → **先把码置为 `InFlight{peer=B}`**（此后任何人再发 `InviteJoin` 都失败），算 SPAKE2，回 `InviteKey{spake: msgA, confirm: cA}`。
3. **确认** B → A：`InviteFinish{confirm: cB}`。
   - A 只认 `env.from == InFlight.peer`，且在 `deadline`（30 秒）内。
   - `cB` 对 → 按撞名规则给 B 编号（沿用 `fd7d901`），签名单 v+1（加上 B），回 `InviteDone{roster}`，然后广播给其余成员。
   - `cB` 不对 / 超时 → `Burned`，回 `InviteFailed`。
   - 两种结果码都作废。

**作废规则：** 码进入 `InFlight` 之后，无论结果都变 `Burned`。第 2 步在进入 `InFlight` 之前失败的（格式错、签名错、B 已在组里）**不**作废 —— 这些不涉及码，R 伪造它们换不来任何关于码的信息。

### SPAKE2 的输入

- 库：`spake2` 0.4（RustCrypto），`Ed25519Group`，非对称模式：A 是 `start_a`，B 是 `start_b`。
- 口令：`field("dct-invite-v1") ‖ field(code 的 6 个 ASCII 数字)`。
- 身份：`idA = rec(A)`，`idB = rec(B)`；`rec` 与现在 `sas.rs` 的一样：名字、端点、签名公钥、加密公钥，逐项 `field` 拼接。
- 共享密钥 `K`（32 字节）。

### 确认值

```
kA, kB = HKDF-SHA256(ikm=K, salt=空, info="dct-invite-v1 confirm A" / "… B")
T  = field("dct-invite-v1") ‖ field(group_id) ‖ field(rec(A)) ‖ field(rec(B))
     ‖ field(msgA) ‖ field(msgB)
cA = HMAC-SHA256(kA, T)      cB = HMAC-SHA256(kB, T)
```

- B 先验 `cA`，不对就停，提示码错（B 那边不必再发 `InviteFinish`；A 会在 `deadline` 后作废）。
- 比较用常数时间（`subtle` 或 `hmac` 的 `verify_slice`）。

### B 收名单

- B 只接受 `InviteDone` 里的名单，且：签名者 == A、签名在 `rec(A)` 里那把签名公钥下成立、A 在名单上且公钥与 `rec(A)` 完全一致、B 在名单上且公钥是自己的（沿用 `roster::accept_invite`，把「确认过的邀请者」换成「PAKE 里绑定的 A」）。
- B 在收到 `InviteDone` 之前不改任何本地状态。

### 版本

- 新增 wire 类型：`InviteProbe` / `InviteOpen` / `NoInvite` / `InviteJoin` / `InviteKey` / `InviteFinish` / `InviteDone` / `InviteFailed`。
- 删掉：`Join` / `JoinPending` / `JoinReveal`，`sas.rs` 整个删掉。
- 守护进程协议号 +1。CLI 已有「任何多电脑命令先查守护进程协议」（`238435a`），老守护进程会被拒绝并提示重启守护进程。
- 两台电脑版本不同：旧版 A 对 `InviteProbe` 不认识 → B 当作 `NoInvite`，提示「对方的 dct 可能太旧，先升级」。

---

## 第 3 段：代码怎么拆、怎么测

### 模块

| 位置 | 内容 |
|---|---|
| `crates/dct-mesh/src/invite.rs`（新） | 纯计算：生成码、包 SPAKE2（身份绑定）、HKDF 出 kA/kB、算/验 cA/cB、`T` 的拼法。不碰网络、不碰时间。 |
| `crates/dct-mesh/src/wire.rs` | 加 8 种、删 3 种消息 |
| `src/mesh/invite.rs`（新） | A 端 `Invite` 状态机（`Live → InFlight → Burned`）；B 端加入流程。都走 `Net` trait。 |
| `src/mesh/mod.rs`、`group.rs` | 删掉 pending 列表、`take_join` / `take_reveal` / `approve`、`MAX_PENDING`、`MAX_CODES_PER_TTL`；`on_envelope` 分派新消息 |
| `src/proto.rs` | 加 `MeshInvite`（回 `{code, expires}`）、`MeshInviteCancel`；`MeshJoin{code, name}`；删 `MeshApprove`、`MeshConfirmInviter`；协议号 +1 |
| `src/mesh/cli.rs` | `dct invite`（打印码和倒计时，Ctrl-C 取消）、`dct join <码> [--name]`（自动登录）；删 `peers approve`、`--confirm` |
| `src/ui/computers.rs` | 看板 `a` 出码 + 倒计时 + 「已加入」「码已作废」提示；删 y/n 行和 `SETTLE` 逻辑 |
| `src/i18n.rs` | 文案（中英） |
| `README.md`、`README.zh-CN.md` | 「多电脑」一节重写 |
| `docs/…/2026-09-28-dct-multi-machine-design.md` | 在被替换的两节顶上加一句「已被 2026-09-30 邀请码设计替换」，不删原文 |

### 测试

- **纯计算（`invite.rs`）**
  - 固定向量：给定 A/B 记录、固定随机种子、码 → `msgA`/`msgB`/`cA`/`cB` 定值。`spake2` 0.4 用自己的 M/N 常数，**不兼容 RFC 9382**，所以没有外部互通向量；这组向量只防我们自己的回归。
  - 码对 → 双方 K 相同、确认通过；码差一位 → 确认失败。
  - 换掉 `rec(A)` 或 `rec(B)` 里任意一项（名字、端点、任一公钥）→ 确认失败。
  - `T` 里换 `group_id` → 失败。
  - 码生成：10⁶ 次采样的分布检查（粗略卡方），且只含数字。
- **假中转对抗（`FakeHub` + 恶意 `Net`）**
  - R 替换 B 的公钥 / A 的公钥 → 不进组，码作废。
  - R 抢先用错码发 `InviteJoin` → 码作废，真 B 随后失败，提示重新生成。
  - R 冒充 A 回 `InviteOpen` / `InviteKey` → B 的 `cA` 验不过。
  - `InviteFinish` 来自非 `InFlight.peer` 的端点 → 忽略，不改状态。
  - 重放上一轮的 `InviteJoin` / `InviteFinish` → 失败。
  - 过期、作废后再试、两台同时发邀请、>16 台同账号在线、B 已在组里。
  - B 在收到 `InviteDone` 之前本地状态不变。
- **端到端**：`tests/mesh_e2e.rs` 改成 `invite → join → peers → send`，真 `dct-srv --with-link --relay-keys` + 两个真守护进程。
- **变异测试**：手工，同上次的做法。重点：「进入 `InFlight` 即作废」、身份绑定、`env.from == InFlight.peer`、常数时间比较、B 提前改状态。

---

## 安全隐患（写文档时补，请用户确认）

和现在比，**变好的**：R 冒充成功率从「每次 10⁻⁶、最多约 20 次」变成「每个码 10⁻⁶、只有一次」；离线试码做不到。

**还在、或者新出现的：**

1. **看到码的人就能加入 —— 但必须是同一个 DC 账号。** 码只要 10 分钟内被别人看到（投屏、截图、背后看），而且那人手里有**你账号**的 DC key，他就能加进来。别的账号的电脑连 `InviteProbe` 都发不到你的电脑（中转按账号隔离）。给学生用时：老师投屏出码，别的学生账号不同，进不来。
2. **老电脑上不再有人点「同意」。** 码就是同意。如果有人在你走开时用你的电脑按了 `a`，再用一台配了你账号的电脑敲码，也能进来。这和「有人拿到你解锁的电脑」是同一级风险。
3. **进组之后权限很大。** 组里任何一台都能给你的会话留言，留言会敲进空闲的智能体里。所以「加入」这道门守的是远程操控你的智能体的权限。这是第一步就有的，邀请码只是让门更好开；`dct peers remove` 仍然是唯一的撤销手段，被移除的电脑令牌最多还能用 7 天（第一步已知限制 2）。
4. **`spake2` 库没有正式审计。** 它是 RustCrypto 的，下载量不小，但 0.4 没有审计报告。缓解：固定版本、跑对抗测试、只用它最基本的非对称接口；以后可以换成审计过的 CPace 实现，协议里的 `dct-invite-v1` 标签留了换算法的余地。
5. **捣乱（DoS）。** R 或同账号的任何一台电脑都可以不停抢先发错码，把你的邀请码一个个烧掉，让你加不进来。换不来冒充，但会烦人。第一版不防，只在看板上提示「有人用错码试过」让用户知道。
6. **多电脑钥匙是明文文件。** `~/.dct/mesh/sign.key`、`kx.key` 是 0600 的文件，不在 Secure Enclave 里（第一步 spec 说要放，代码没放）。本设计不改，但和第 3 条叠在一起：谁拿到这两个文件，谁就是组里那台电脑。
7. **并发加入会让名单分叉**（第一步已知限制 1）：两台老电脑同时各拉一台新电脑进来，各签一个 v+1。本设计不改。

---

## 不做的

- 不用配 DC 账号就能加入（邀请码换中转令牌）—— 要改网关，单独设计。
- 中转侧限速。
- 把钥匙放进 Secure Enclave。
- 解决名单分叉。
