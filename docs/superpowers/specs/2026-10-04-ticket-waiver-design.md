# 放行凭据：用户确认过的规矩，让章鱼自己点没有字的位置

日期：2026-10-04
状态：设计，待 dc-octo 审；没有代码。用户 2026-10-04 拍板：选「规矩放行，自动钥匙签票」，并说「你必须想办法让章鱼点」。
背景：dco 有一条根本规矩：没有字的东西属于「内容档」，没有票一律不点；dco 也特意拒绝把划动当点击（`swipe` 至少划出窗口宽 2%）。要让章鱼点游戏场景里的物件，需要一个**按坐标点**的工具（`tap_at`），而且点之前要有用户事先确认过的依据。用户已经在 dcv 里确认了《Darkness and Flame》的一块可点区域（`games:darkness-and-flame-rules`，`tap_allow: no_text 0.03 0.12 0.75 0.66`），但 dco 没有任何东西读它，`dco` 也不碰 dcv（用户定的边界），所以由 dct 在票上带上依据。

## 现状（核对过代码）

- `crates/dct-brain/src/tier.rs`：`Tier::rule_can_waive()` 只是个判断函数，只有测试用它；`required_signer()` 对 `Content` 要 `User`。
- `crates/dct-brain/src/sign.rs`：`role_may_sign(tier, role)` 对 `Content` 只许 `User`；`verify` 因此拒绝自动钥匙签的内容档票。
- `ticket.rs`：`Subject::Action { tool, args_sha256 }` 和 `args_sha256()` 已有。
- 所以「规矩放行 → 自动钥匙签内容档票」现在签不出、也过不了验。

## 设计

### 1. 票多一个可选的放行凭据（dct-brain，dct 和 dco 共用）

```rust
pub struct Waiver {
    /// 放行的是哪一类：现在只有 "no_text"（一块没有字的空白）。
    pub kind: String,
    /// 哪条规矩放行的，`库:名字`。
    pub rule: String,
    /// 那条规矩被用户确认时的正文指纹（dcv 的 `sha`）。规矩改一个字，这个指纹就变，旧票作废。
    pub rule_sha256: String,
    /// 用户确认规矩的时间（dcv 的 `approved_at`）。
    pub approved_at: String,
    /// 规矩里放行的那块区域，**整数万分比**（0–10000，窗口左上角为原点）。dct 把它签进票；
    /// dco 不读 dcv，但会机械地检查「点落在票里签的区域里」，并把整个凭据写进事件记录。
    pub region: Region,
}
pub struct Region { pub x_bp: u16, pub y_bp: u16, pub w_bp: u16, pub h_bp: u16 }
// Ticket 加：pub waiver: Option<Waiver>
```

- **`canonical_bytes` 只在有放行凭据时才追加**一个 `"waiver"` 标记和四个字段；**没有凭据的票，字节和指纹和现在逐字节相同**（旧票、旧测试、旧签名全部照旧有效）。
- `TICKET_VERSION` 不变（字段可选、不影响旧票）。

### 2. 谁能签、谁能验

- `role_may_sign(tier, role, waiver)`：
  - 现有规矩不变；
  - **新增唯一一条例外：** `tier == Content`，`tier.rule_can_waive()` 为真，且票带了 `waiver`，且 `waiver.kind == "no_text"` → 允许 `Auto` 签。
  - `Critical`、`Money` 永远不放行（`rule_can_waive()` 本来就是假）；`Content` 但没带凭据，还是只许 `User`。
- `verify` 对应放行：带凭据的 `Content` 票接受 `Auto` 签名，其余检查（签名、设备、有效期、nonce 一次性、角色够不够）一个都不少。

### 3. 谁来决定「这一点能点」

- **dct 来判断，不是 dco。** dct 在 `dcv games --json --app <应用>` 里找一条 `rules.approved_at` 非空、`tap_allow` 里有 `no_text` 区域、且点（窗口 0–1）落在区域内的规矩，才签。不满足就不签（不去问用户，也不绕）。
- 签的票：`Subject::Action { tool: "tap_at", args_sha256 }`，`tier: Content`，`waiver: Some(...)`，**有效期 30 秒**，一次性 nonce，`device` 是目标 dco 的设备名。
- `args` 对象（不含 ticket）：`{"window":{"window_id":N},"x_bp":0..10000,"y_bp":0..10000}`，用 `ticket::args_sha256` 算。**点击位置用整数万分比**，避免浮点两边表示不一致。

### 4. dco 的底线（不被票绕过）（dc-octo 审定，2026-10-04）

「落点读出任何字」的定义：dco 读落点周围一小块（窗口宽 6% × 高 3%，和 `tap` 判档一致），只有「文字框盖住落点、或离落点不到窗口宽 1.5%」的字才算「落点上有字」；置信度太低、或不足 2 个字母数字的当噪点忽略（游戏里的血条、伤害数字、HUD 不会误拒）。带放行凭据的票读到字 → `not_allowed`；不带凭据、由用户本人签的 content 票读到字 → 按字定档，票档位不低于它就点（和 `tap` 一样）。钱档字：任何票都拒绝，除非票本身是 Money 档、用户签的。换算：点 = 窗口左上角 + (x_bp/10000 × 窗口宽, y_bp/10000 × 窗口高)；`args` 必须正好只有 `window`（里面只有 `window_id`）、`x_bp`、`y_bp` 三个键，多一个键就 `bad_request`；`window_id` 会变（iPhone 镜像重连后换过），dct 每次签之前用 `list_windows` 现取。dco 的 `verify` 复用 `dct_brain` 的（同一个 crate）。点击时序和 `tap` 一样（iPhone 镜像用长按），章鱼进入「操作」。（以下是原来的内容。）

dco 点之前先用本机文字识别读落点周围一小块：
- **带放行凭据的票只放行「没有字」的落点。** 落点读出任何字，一律拒绝（`not_allowed`，说明那里写着什么字、放行凭据只管空白），**不论是什么字**。这样自动钥匙签的放行票，永远点不到「发布」「发送」「确认」这类带字按钮。
- 读出价格、购买、付款这类字（钱档）：拒绝，任何票都不行。
- 点在窗口外：`bad_request`。
- 返回 `{tapped:{at:{x_bp,y_bp}, kind:"no_text"}}`，章鱼进入「操作」。
- 错误码：`needs_ticket`、`ticket_rejected`（带人话原因：过期、用过、设备不对、凭据不被接受、参数对不上等）、`not_allowed`、`bad_request`。

### 5. 为什么这样不削弱安全

- **放行范围极窄：** 只有 `Content` 档、只有 `no_text` 这一类、只有一次点击、30 秒有效。
- **规矩改了票就废：** 凭据里带规矩的指纹，用户在 dcv 里改了规矩要重新确认，旧票不再被 dct 签（dct 签之前现读一次 `dcv games`）。
- **dco 不信「区域」：** dco 不读 dcv，也不判断区域；它只做自己能独立做的事：验签名、验凭据类型、读落点有没有字。区域这一层由 dct 把关，dct 本来就持有自动钥匙（物理档的划动、点击已经由它签）。
- **残余风险（缩小后）：** 凭据里签进了区域，dco 机械地检查「点落在区域里」。拿到自动钥匙的程序要点区域外的没字位置，必须**把一个错的区域签进票**，对账时看得出来。（原来的说法：拿到自动钥匙的程序可以点任何没有字的位置。）这和现在自动钥匙能签物理档票（点任何带字的「自用」按钮）是同一等级的信任，不是新增的等级；但要明说。缓解：`Waiver` 里带规矩名和指纹，dco 的事件记录会写下来，事后可对账；区域本身以后可以让 dct 把它也签进票（作为 `Waiver` 的字段），让 dco 至少记下区域，仍然不由 dco 判断。

## 要做的事

**dct 这边，分两半：**
- **先合到 main 的一半（`crates/dct-brain`，纯函数，dco 编译要用）：** `Waiver`、`Region`、`Ticket.waiver`、`canonical_bytes` 的追加规则、`role_may_sign`/`verify` 的例外、一个纯函数 `waiver_for_tap(rule, x_bp, y_bp) -> Result<Waiver, Refusal>`（点在区域内才给出凭据）。
- **之后（`src/`，要用密钥和 dcv）：** 读 dcv 的已确认规矩、`list_windows` 取窗口号、用自动钥匙签单动作票。

**dct 这边（详细）：**
1. `Waiver` 类型、`Ticket.waiver`、`canonical_bytes` 的追加规则（无凭据逐字节不变，用测试钉住）。
2. `role_may_sign` / `verify` 的那一条例外，和 `Critical`/`Money`/无凭据的拒绝测试。
3. 签票：给定点和窗口，查 dcv 的已确认规矩，在区域内才签，返回 `SignedTicket`；不在区域、规矩没确认、规矩改过，都不签。
4. `dct game scene` 里调用（后面的功能）。

**dco 那边（dc-octo）：**
1. 认 `Waiver`（类型、`kind` 限制）、`verify` 对应放行。
2. `tap_at{window, x_bp, y_bp, ticket}`：验票、读落点、返回。

## 测试（dct 这一半）

- 无凭据的票：指纹逐字节同今天（用现有测试向量）；
- 带凭据的 `Content` 票：`Auto` 签的通过 `verify`；`User` 签的也通过；
- 带凭据但 `kind != "no_text"`：`Auto` 签的拒绝；
- `Critical`、`Money`：带凭据也拒绝 `Auto`；
- 没带凭据的 `Content` 票：`Auto` 签的拒绝（现有行为不变）；
- 凭据里的规矩指纹被改：dct 重签时拒绝（规矩改了）；
- 点在区域外 / 规矩没确认 / 没有这条规矩：不签；
- 同一个 nonce 不能用第二次（沿用现有）。

## 不做

- dco 读 dcv；
- 放行带字按钮；放行钱档和不可撤回档；
- 多次点击一张票。
