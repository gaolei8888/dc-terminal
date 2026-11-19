# dct 远程操作（第一期）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在一台电脑的 dct 看板上看见并操作同组另一台电脑（开了开关的）上的会话，跟本机一样。

**Architecture:** 看板只跟本机守护进程说话；本机守护进程把 `Request` 封进 `Kind::RemoteCall` 信封经现有中转 `ask` 对方；对方守护进程验名单、查开关和白名单，在每台操作方各一条的工作线程里执行，按 `seq` 去重，经 `Net::reply` 回答。看板用一个路由层把远程会话映射成本地的「合成 id」，画面读后台线程的缓存、按键进后台队列，主循环永不等网络。

**Tech Stack:** Rust；`dct_mesh`（seal/wire/roster）、`src/mesh`、`src/link.rs`、`src/daemon.rs`、`src/ui/*`；测试用 `mesh::net::FakeHub`。

**Spec:** `docs/superpowers/specs/2026-11-19-dct-remote-control-design.md`

## Global Constraints

- 开关默认关；只能从本机改（`Origin::Local`）。
- 远程白名单（spec 第 2 节表格原样）：View = `Hello List Screen Screens Explanation Projects Profiles`；Operate = `Input Submit Key Mouse Scroll Resize`；Manage = `Create Stop Undo Diff LastProfile`（`Create.dir` 必须在对方 `Projects` 里）。其余一律拒绝，新加的 `Request` 默认拒绝。
- `seq`：每台操作方电脑对每台被操作方一条递增序号；同一 `seq` 只执行一次，重复到达回上次答复；重试用新信封、同一 `seq`。
- 在场：只有 `Screen`/`Screens` 和 Operate/Manage 请求算，`List` 不算；窗口 10 秒；提示画在看板顶栏和会话底栏。
- 踢掉键 `K`（看板、九宫格都认）= 关开关 + 清在场。
- 按键：普通字符合并成一次 `Input`；失败带原 `seq` 重试；连续失败 3 次清空队列并报「发往 {电脑} 的按键没送到」。
- 远程 F5 只粘文字；底栏说明。
- 不改中转（`crates/dct-srv`）。
- `PROTOCOL_VERSION` 26 → 27。
- 提交信息不加 Co-Authored-By（用户要求）。

## Review Focus

1. 开关关着时，任何远程请求（包括白名单内的）都不执行——Task 3 测试。
2. 被操作方慢（执行一条要 3s）时，同时到的留言照常送达——Task 3 测试。
3. 远程目标卡死时，本机看板一帧不超过 50ms——Task 5 测试。
4. 同一 `seq` 到两次，`Create` 只建一个会话——Task 3 测试。
5. 老版本对方（解不开 `RemoteCall`）→ 本机报「连不上」而不是卡住——Task 4 测试（假 `Net` 对该端点永远超时）。

---

### Task 1: 线上格式、授权判定、错误码

**Files:**
- Modify: `crates/dct-mesh/src/seal.rs`（`Kind` 加 `RemoteCall`、`RemoteReply`）
- Create: `src/mesh/remote.rs`（`pub mod remote;` 加进 `src/mesh/mod.rs`）
- Modify: `src/proto.rs`（`ErrorCode` 加三种；`PROTOCOL_VERSION = 27`；`Request::RemoteCall`、`Request::RemoteControl`；`Response::RemoteControl`）
- Modify: `src/i18n.rs`（三条错误文案中英）

**Interfaces:**
- Produces:
  - `pub enum Capability { View, Operate, Manage }`
  - `pub struct Grant { pub who: String, pub project: Option<PathBuf>, pub caps: Vec<Capability> }`
  - `pub fn capability_of(req: &Request) -> Option<Capability>`（白名单外 `None`）
  - `pub fn allowed(req: &Request, grants: &[Grant], from: &str, projects: &[PathBuf]) -> Result<(), ErrorCode>`（`Create` 查 `dir ∈ projects`；无 grant → `RemoteControlOff`；有 grant 无能力或白名单外 → `RemoteForbidden`）
  - `pub fn grants_for(switch_on: bool, roster: &Roster, me: &str) -> Vec<Grant>`（开：除自己外每台一条全能力、`project: None`；关：空）
  - `#[derive(Serialize, Deserialize)] pub struct CallBody { pub seq: u64, pub req: Request }`、`pub struct ReplyBody { pub seq: u64, pub resp: Response }`
  - `ErrorCode::RemoteControlOff`、`ErrorCode::RemoteUnavailable { peer: String }`、`ErrorCode::RemoteForbidden`
  - `Request::RemoteCall { peer: String, req: Box<Request> }`；`Request::RemoteControl { set: Option<bool> }` → `Response::RemoteControl { on: bool, viewers: Vec<String> }`
  - `pub fn counts_as_presence(req: &Request) -> bool`（`Screen`/`Screens`/Operate/Manage 为真，`List` 等为假）

- [ ] **Step 1: 写失败测试**（`src/mesh/remote.rs` 内 `mod tests`）

```rust
#[test] fn every_whitelisted_request_maps_to_its_capability()   // 表格逐条断言
#[test] fn requests_off_the_list_are_refused() {
    // Kill, Prune, SetSecret, PairStart, PhoneUnpair, WebEnable, LiveStart, MeshInvite,
    // Drain, PinProject, RemoteControl{set:Some(true)}, RemoteCall{..} → capability_of == None
}
#[test] fn switch_off_refuses_everything()      // grants_for(false,..) → allowed(List) == Err(RemoteControlOff)
#[test] fn a_sender_not_on_the_roster_is_refused() // grants 不含它 → Err(RemoteControlOff)
#[test] fn create_outside_the_peers_projects_is_forbidden() // Err(RemoteForbidden)
#[test] fn listing_is_not_presence_but_watching_and_typing_are()
#[test] fn call_and_reply_bodies_round_trip_through_json()
```

`proto.rs` 里现有的「全部请求 JSON 形状」快照测试（`proto.rs:1703` 那条）补上两条新请求。

- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test --lib mesh::remote` Expected: 编译失败（模块不存在）

- [ ] **Step 3: 实现** — `capability_of` 用穷尽 `match`（不写 `_ =>` 兜底到 Some），新加的 `Request` 变体编译期就逼人表态。

- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test --lib mesh::remote proto::` Expected: PASS

- [ ] **Step 5: Commit** — `git commit -m "feat(mesh): remote-call wire format and the remote whitelist"`

### Task 2: 开关落盘 + 请求来源 `Origin`

**Files:**
- Modify: `src/mesh/store.rs`（`remote_control.json`：`{"on": bool}`；缺文件 = 关）
- Modify: `src/daemon.rs`（`handle` 加参数 `origin: Origin`；`Request::RemoteControl` 只在 `Origin::Local` 下可 `set`；所有现有调用点传 `Origin::Local` 或 `Origin::Http`）

**Interfaces:**
- Consumes: Task 1 `capability_of`、`Request::RemoteControl`
- Produces: `Store::remote_control(&self) -> bool`、`Store::set_remote_control(&self, on: bool) -> Result<()>`（原子写，同 `save_roster` 的 tmp+rename）；`pub enum Origin { Local, Http, Remote { from: String } }`（`src/daemon.rs`）

- [ ] **Step 1: 写失败测试**

```rust
#[test] fn remote_control_is_off_until_turned_on_and_survives_reload() // store.rs
#[test] fn remote_origin_cannot_toggle_remote_control()   // handle(RemoteControl{set:Some(true)}, Origin::Remote{..}) → Error RemoteForbidden，开关仍关
#[test] fn remote_origin_is_refused_off_whitelist_even_inside_handle() // handle(Kill{id}, Origin::Remote{..}) → RemoteForbidden（第二道）
#[test] fn http_origin_cannot_toggle_remote_control()
```

- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test --lib remote_control remote_origin http_origin` Expected: 编译失败

- [ ] **Step 3: 实现** — Ruling：开关存 `~/.dct/mesh/remote_control.json`，不存 spec 写的 `config.toml [mesh] allow_remote_control`——`config.toml` 现在没有任何代码写它（只读），程序改写会冲掉用户的注释；mesh store 已有原子写。错了的代价：用户手改 `config.toml` 不生效，需在 README 写明开关在设置页。Ruling 已定：保留现有 `web: Option<…>` 参数不并入 `Origin`（spec 说「我的决定、不并也行」；并入会碰 `web.rs` 全部路由，留到以后）。`Origin::Remote` 进 `handle` 第一行就过 `capability_of`。

- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test --lib` Expected: PASS

- [ ] **Step 5: Commit** — `git commit -m "feat(daemon): remote-control switch and request origin"`

### Task 3: 被操作方：收、验、排队执行、去重、在场

**Files:**
- Modify: `src/mesh/net.rs`（`Net` 加 `fn reply(&self, to: &str, seq: u64, payload: Vec<u8>) -> Result<(), LinkError>`；`LinkNet` 用 `link::send` 发 `Envelope { seq, .. }`；`FakeHub` 支持「先不回、稍后 reply」：`ask` 在目标 `on_envelope` 返回 `None` 时等同 `seq` 的 `reply` 到 `timeout`）
- Create: `src/mesh/remote_host.rs`
- Modify: `src/mesh/mod.rs`（`take_sealed` 处理 `Kind::RemoteCall`；新 `Step::Remote`）
- Modify: `src/mesh/deliver.rs`（`Inbox` 加 `fn remote_call(&self, from: &str, req: Request) -> Response`；`fn projects(&self) -> Vec<PathBuf>`）
- Modify: `src/daemon.rs`（实现两条新 `Inbox` 方法：`remote::allowed` → `handle(.., Origin::Remote{from})`）

**Interfaces:**
- Consumes: Task 1 全部；Task 2 `Store::remote_control`、`Origin`
- Produces:
  - `pub struct RemoteHost`：`fn submit(&self, from: &str, env_seq: u64, body: CallBody)`（非阻塞）；`fn viewers(&self, now: Instant) -> Vec<String>`（10 秒窗口，只记 `counts_as_presence` 为真的）；`fn kick_all(&self)`（清在场、清队列）
  - 每台操作方一条工作线程；队列里 Operate/Manage 先于 View；新 View 到达时丢掉队里旧的 View
  - 去重：每台操作方记 `applied: u64` 和最近 64 条 `(seq, Response)`；`seq <= applied` 且在缓存里 → 直接回缓存；`seq <= applied` 不在缓存里 → 回 `RemoteUnavailable`（太旧）
  - 回答：`Net::reply(from, env_seq, wire::encode(Sealed(RemoteReply{seq, resp})))`

- [ ] **Step 1: 写失败测试**（`FakeHub` 上两台 `Mesh`，B 的 `Inbox` 是假实现，记录 `remote_call` 被调了几次）

```rust
#[test] fn a_remote_list_comes_back_when_the_switch_is_on()
#[test] fn switch_off_answers_remote_control_off_and_runs_nothing()   // Review Focus 1
#[test] fn the_same_seq_twice_runs_once_and_answers_the_same()        // Review Focus 4：Create 只调一次
#[test] fn a_slow_remote_call_does_not_hold_up_a_message()            // Review Focus 2：remote_call 睡 3s，同时 MeshSend 的留言 1s 内被 type_into
#[test] fn keys_jump_ahead_of_queued_screens_and_old_screens_are_dropped()
#[test] fn presence_counts_watching_and_typing_not_listing()          // 只发 List → viewers 空；发 Screen → [A]；11s 后空
#[test] fn a_replayed_envelope_is_dropped()                           // 同一信封原样再投 → 去重表挡掉
#[test] fn kick_all_clears_viewers()
```

- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test --lib mesh::` Expected: 编译失败 / 新测试 FAIL

- [ ] **Step 3: 实现** — `take_sealed`：`RemoteCall` 先用远程专用去重表（不进 `Seen`）；开关关 → 立即 `reply(RemoteReply{resp: Error(RemoteControlOff)})`（同步答复，`on_envelope` 返回 `Some`）；开 → `Step::Remote`，`on_envelope` 在锁外 `remote_host.submit(..)` 并返回 `None`（不占收件线程）。答复超过 `MAX_PAYLOAD`：`Diff` 截断到 1MB 以内并在末尾加一行「（太长，已截断）」，`Screens` 只回能装下的前几格、其余回 `RemoteUnavailable`（Ruling 记账）。

- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test --lib` Expected: PASS

- [ ] **Step 5: Commit** — `git commit -m "feat(mesh): serve remote calls off the link thread, once per seq"`

### Task 4: 操作方守护进程：`Request::RemoteCall`

**Files:**
- Modify: `src/mesh/mod.rs`（`Mesh::call_remote(&self, peer_name: &str, req: Request, net: &dyn Net) -> Response`；按电脑名查名单；每个被操作方一条 `next_seq`）
- Modify: `src/daemon.rs`（`Request::RemoteCall` 只在 `Origin::Local` 下处理；`Request::RemoteControl` 读写开关、带 `viewers`；`dct status --json` 加 `remote_viewers`）
- Modify: `src/cli.rs`（`status --json` 输出字段）

**Interfaces:**
- Consumes: Task 1 `CallBody`/`ReplyBody`；Task 3 `Net::reply`、`RemoteHost::viewers`
- Produces: `Mesh::call_remote`：分配 `seq` → 封信 → `net.ask(.., CALL_TIMEOUT)`；超时/离线重试至多 2 次（新信封同 `seq`）；最终失败 → `Response::Error(RemoteUnavailable{peer})`；对方答复原样返回

- [ ] **Step 1: 写失败测试**

```rust
#[test] fn remote_call_round_trips_a_screen()
#[test] fn a_timeout_is_retried_with_the_same_seq()          // 假 Net 第一次 ask 超时但 B 已执行 → 第二次拿到缓存答复，B 只执行一次
#[test] fn an_old_peer_that_never_answers_is_reported_unavailable() // Review Focus 5
#[test] fn an_unknown_peer_name_is_reported_unavailable()
#[test] fn remote_call_from_a_remote_origin_is_refused()     // 不许经 A 转跳到 C
#[test] fn status_json_lists_remote_viewers()
```

- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test --lib remote_call status_json` Expected: FAIL

- [ ] **Step 3: 实现** — 看板到守护进程的 `RemoteCall` 要用 `Client::call_within(.., 30s)`（三次 8s 加余量）；这一条在 Task 5 的路由层里用，这里只保证守护进程侧不早于 30s 放弃。

- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test` Expected: PASS（已知 flaky 见 memory，单独重跑确认）

- [ ] **Step 5: Commit** — `git commit -m "feat(daemon): call a peer's daemon through the relay"`

### Task 5: 看板路由层 + 后台线程（主循环永不等网络）

**Files:**
- Create: `src/ui/remote.rs`
- Modify: `src/ui/app.rs`（`app.client()` 返回 `&mut Router`；`Router` 包住原 `Client`）
- Modify: `src/session.rs`（`SessionInfo` 加 `#[serde(default)] pub machine: Option<String>`，只由界面填）

**Interfaces:**
- Consumes: Task 4 `Request::RemoteCall`
- Produces:
  - **合成 id（Ruling：替代 spec 里的 `SessionKey` 迁移——spec 把切分方式留给实现者；合成 id 让 `View::Attached(u32)` 等 47 处不动）**：`const REMOTE_BASE: u32 = 1 << 24`；`Router` 维护 `(peer, real_id) ↔ synthetic_id` 双向表，只增不减（会话没了 id 也不复用）
  - `pub struct Router { local: Client, remotes: HashMap<String, Feed> }`，`pub fn call(&mut self, req: Request) -> Result<Response>`：
    - 不含合成 id 的请求 → 原样给本机
    - `List` → 本机列表 + 每个 `Feed` 缓存的远程列表（id 换成合成 id，`machine = Some(peer)`）
    - 合成 id 的 `Screen`/`Screens` → 立刻返回 `Feed` 缓存（没有缓存 → 空屏 + 「正在连接 Mac…」）
    - 合成 id 的 `Input/Submit/Key/Mouse/Scroll/Resize/Create/Stop/Undo/Diff` → 进 `Feed` 队列，立刻返回 `Ok`（`Diff`、`Undo` 的结果走 `Feed` 回调进 `app.message`）
  - `pub struct Feed`：一条后台线程 + 自己的 `Client`；每 3s `RemoteCall{List}`；`watch(ids)` 设置当前要刷画面的远程会话，循环取；按键队列合并普通字符、失败 3 次清空并发出 `FeedEvent::KeysLost`；`fn age(&self, id) -> Option<Duration>` 给「n 秒前」
  - 远程 F5：`Router` 收到合成 id 的粘贴图片请求 → `Err(RemoteForbidden)`，底栏文案「远程会话只能粘文字」

- [ ] **Step 1: 写失败测试**（`Router` 用假 `Client` 和假远程，远程每次调用睡 1s）

```rust
#[test] fn local_requests_pass_straight_through()
#[test] fn a_slow_remote_never_blocks_a_frame()          // Review Focus 3：连续 50 次 Screen(合成 id) 总耗时 < 50ms
#[test] fn plain_characters_coalesce_control_keys_do_not() // "a","b","c",Enter,"d" → Input("abc"), Key(Enter), Input("d")
#[test] fn keys_keep_their_order_across_a_retry()
#[test] fn three_failures_drop_the_queue_and_say_so()     // FeedEvent::KeysLost
#[test] fn list_merges_remote_sessions_with_synthetic_ids_and_machine_names()
#[test] fn an_offline_peer_keeps_its_sessions_marked_offline()
#[test] fn synthetic_ids_are_never_reused()
```

- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test --lib ui::remote` Expected: 编译失败

- [ ] **Step 3: 实现** — `Feed` 的线程包 `catch_unwind`（同 `bridge.rs::spawn` 的规矩）；`Router::call` 里不得出现任何会阻塞在网络上的调用（审查点）。

- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test --lib` Expected: PASS

- [ ] **Step 5: Commit** — `git commit -m "feat(ui): route remote sessions through background feeds"`

### Task 6: 看板上的远程：分组、进会话、开关、提示、踢掉

**Files:**
- Modify: `src/ui/view.rs`（`group_sessions`：`machine = Some(name)` 的会话按电脑名分组，排在本机分组之后；组头写电脑名，行里写项目路径；离线的组灰掉并写「离线」；没开远程的写「没开远程操作」）
- Modify: `src/ui/attach.rs`、`src/ui/grid.rs`、`src/ui/board.rs`（合成 id 进会话时 `Router::watch`；画面缓存超 2s 角上写「n 秒前」；`K` 键）
- Modify: `src/ui/settings_view.rs`（一项「允许我的其他电脑远程操作」，读写走 `Request::RemoteControl{set}`）
- Modify: `src/ui/mod.rs`（顶栏和会话底栏的在场提示：每 3s `RemoteControl{set: None}` 取 `viewers`）
- Modify: `src/i18n.rs`、`README.md`、`README.zh-CN.md`（「多电脑」一节加「远程操作」，并把「还不能进入别的电脑上的会话」那条删掉）

**Interfaces:**
- Consumes: Task 5 `Router`、`Feed::age`、`FeedEvent`；Task 4 `Request::RemoteControl`

- [ ] **Step 1: 写失败测试**（沿用现有 `view.rs`/`grid.rs` 的纯函数测试套路）

```rust
#[test] fn remote_sessions_group_under_their_machine_after_local_ones()
#[test] fn an_offline_machine_group_is_dimmed_and_labelled()
#[test] fn k_turns_remote_control_off()                     // 发出 RemoteControl{set: Some(false)}
#[test] fn the_presence_banner_shows_on_the_board_and_inside_a_session()
#[test] fn a_stale_remote_screen_says_how_old_it_is()        // age 3s → 角上「3 秒前」
#[test] fn f3_rotates_through_remote_sessions_too()
#[test] fn n_on_a_remote_group_creates_on_that_machine()     // 发出 RemoteCall{peer, Create{..}}
```

- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test --lib ui::` Expected: FAIL

- [ ] **Step 3: 实现**

- [ ] **Step 4: 跑全量测试 + clippy** — Run: `cargo test --no-fail-fast && cargo clippy --all-targets` Expected: PASS，0 warning

- [ ] **Step 5: 实机验证**（**先问用户**：要两台电脑、要 `dct restart` 两边）——这台 `dct` 设置里开开关；另一台看板上出现「这台电脑名」分组；进一个会话打字、看回显；这台看板顶栏亮提示；按 `K` 后那台报「没开远程操作」。

- [ ] **Step 6: Commit** — `git commit -m "feat(ui): remote machines on the board — enter, type, watch, kick"`
