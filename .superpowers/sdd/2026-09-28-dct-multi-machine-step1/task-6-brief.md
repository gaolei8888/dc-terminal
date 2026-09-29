### Task 6: 登录、加入、批准、移除（命令行 + 协议）

**Files:**
- Create: `src/mesh/cli.rs`
- Modify: `src/proto.rs`
  - 新请求：`MeshStatus`、`MeshLogin`、`MeshJoin { name: String }`、`MeshApprove { endpoint: String, yes: bool }`、`MeshRemove { name: String }`
  - 新响应：`Response::Mesh(MeshView)`
  - `PROTOCOL_VERSION` 加一，并更新 `the_request_shape_is_pinned_to_the_protocol_version` 那一类守卫测试
- Modify: `src/daemon.rs`（`handle` 接新请求）
- Modify: `src/main.rs`：加 `Some("login")`、`Some("join")`、`Some("peers")`、`Some("send")`，都转到 `mesh::cli::run(&args)`
- Modify: `src/mesh/mod.rs`

**Interfaces:**
- Produces:
  - `proto::MeshView`：
    - `logged_in: bool`
    - `name: String`
    - `endpoint: String`
    - `in_group: bool`
    - `members: Vec<MemberView>`，其中 `MemberView { name, endpoint, online: bool, is_me: bool }`
    - `pending: Vec<PendingJoin>`，其中 `PendingJoin { name, endpoint, code }`
  - `mesh::cli::run(args: &[String]) -> i32`

**流程**（每一步都有对应的测试，用假的中转连接；`Link` 的 send/ask/peers 通过 `Mesh` 里一个可注入的 `Net` trait 走）：
1. `dct login`：
   - 读 secrets 里 `"dc"` 的 api_key。没有就提示：「先在 dct 里用 DC 配对账号（进 dct 按 c 选 DC）」，退出码 1。
   - 调 `fetch_token` 换令牌，存进 `__relay__`，通知守护进程开始连接。
   - 名单还不存在时：`roster::genesis(me, group = "mine-" + endpoint, keys)`，保存，打印「这台电脑成了「我的电脑」组的第一台」。
2. `dct join [--name 公司Windows]`，在新电脑上运行，前提是已经 login、还不在组里：
   - 可以顺手改名；
   - 向 `link::peers` 返回的每个在线端点发 `Payload::Join(JoinRequest)`；
   - 打印：「请在你已有的任意一台电脑上看一眼：那边会显示一个 6 位数，跟这里的 ______ 一样就点同意」。这个 6 位数是用 `sas::code(我, 对方)` 对每个在线端点各算一个；对方可能有好几台，就逐台列出「电脑名：数字」；
   - 等名单到来（最多 10 分钟）。收到的 `Payload::Roster` 包含我、而且 `roster::accept(None 或 当前)` 通过，就保存，打印「已加入」。

   注意：新电脑没有旧名单，它接受的第一份名单只要求**签名者就是给自己算过 6 位数的那台电脑**，并且名单里包含自己。
3. 已有的电脑收到 `Join`：
   - 验 `JoinRequest` 自签名；
   - 算出 6 位数；
   - 放进 `pending_joins`，TUI 和 `dct peers` 里都显示；
   - 回一个 `Payload::JoinPending`。
4. `dct peers approve <电脑名或 endpoint> [--no]`，或者在 TUI 里按 y：
   - `MeshApprove`：新名单 = 旧名单加上这台，`version + 1`，用本机钥匙签；
   - 保存，然后发给所有在线成员，加上新电脑；
   - 拒绝时只删掉 pending。
5. `dct peers remove <电脑名>`：
   - 新名单去掉这台，`version + 1`，签名并广播；
   - 不能移除自己，给出提示。

**给用户看的话**（全部走 i18n，在 `src/i18n.rs` 加 key 或 `msg` 函数）：
- 「一台叫 {name} 的电脑想加入「我的电脑」。它屏幕上的数字是 {code} 吗？(y/n)」
- 「{name} 已加入」
- 「{name} 已移出」
- 「还没登录多电脑，先运行 dct login」

- [ ] **Step 1: 失败的测试**
  - 用两个、三个内存里的 `Mesh` 加一个假 `Net`，跑完整的「A login → B login、join → A approve → B 已加入 → C join → B 批准 → A 也收到新名单」；
  - 中途伪造一个 Join：公钥被换了，6 位数就对不上；只要没人批准，它就进不了名单；
  - `remove` 之后，被移除的电脑再发 Sealed 会被丢弃；
  - `PROTOCOL_VERSION` 守卫。
- [ ] **Step 2–4: 实现、通过**，外加 Windows 检查
- [ ] **Step 5: Commit** —— `git commit -m "feat(mesh): dct login, join with a 6-digit compare, approve and remove"`

---


## Controller rulings (override the brief)
1. JoinPending is `{ member: Member, sig }` (see Task 2). The joining machine computes `sas::code(me, pending.member)` per reply and prints "电脑名：数字" for each.
2. Add to dct-mesh `roster::accept_invite(incoming: &SignedRoster, me_endpoint: &str, inviter: &Member) -> Result<(), RosterError>`: signer == inviter.endpoint; signature valid under the inviter's sign_pub; roster contains both me and the inviter; no duplicate names/endpoints. The joining machine uses it for its first roster (inviter = the member whose JoinPending it displayed). Tests for accept_invite in dct-mesh.
3. Use `mesh::Net` / `FakeNet` from Task 5 for all multi-machine tests.
